/// 数据读写会话：与网络协议交互
/// UDP：三次心跳超时则移除会话
/// TCP：连接断开或三次心跳超时则移除会话
pub mod rw {
    use std::collections::{BTreeSet, HashMap};
    use std::fmt::format;
    use std::sync::{Arc, LazyLock, OnceLock};

    use std::time::Duration;

    use parking_lot::Mutex;
    use rsip::{Request, Response, SipMessage};

    use crate::gb::depot::{Callback, SipPackage, default_log_callback};
    use crate::storage::entity::GmvDevice;
    use anyhow::anyhow;
    use base::bytes::Bytes;
    use base::constructor::New;
    use base::exception::GlobalError::SysErr;
    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::{debug, error, warn};
    use base::net::state::{Association, Event, Package, Protocol, Zip};
    use base::once_cell::sync::Lazy;
    use base::tokio;
    use base::tokio::sync::mpsc::{Receiver, Sender};
    use base::tokio::sync::{Notify, mpsc};
    use base::tokio::time;
    use base::tokio::time::Instant;
    use base::tokio_util::sync::CancellationToken;
    use base::utils::rt::GlobalRuntime;

    static RW_CTX: OnceLock<RWContext> = OnceLock::new();

    pub struct RWContext {
        shared: Arc<Shared>,
        //更新设备状态
        db_task: Sender<String>,
        io_tx: Sender<Zip>,
        sip_tx: Sender<SipPackage>,
    }

    impl RWContext {
        pub fn get_ctx() -> &'static RWContext {
            RW_CTX.get().expect("RWContext not initialized")
        }
        pub fn init(io_tx: Sender<Zip>, sip_tx: Sender<SipPackage>) {
            let (tx, rx) = mpsc::channel(16);
            let session = RWContext {
                shared: Arc::new(Shared {
                    state: Mutex::new(State {
                        sessions: HashMap::new(),
                        expirations: BTreeSet::new(),
                        bill_map: HashMap::new(),
                    }),
                    background_task: Notify::new(),
                }),
                db_task: tx.clone(),
                io_tx,
                sip_tx,
            };
            let shared = session.shared.clone();
            let rt = GlobalRuntime::get_main_runtime();
            rt.rt_handle
                .spawn(Self::do_update_device_status(rx, rt.cancel.clone()));
            rt.rt_handle
                .spawn(Self::purge_expired_task(shared, rt.cancel));
            let _ = RW_CTX.set(session);
        }
        async fn do_update_device_status(
            mut rx: Receiver<String>,
            cancel_token: CancellationToken,
        ) {
            while let Some(device_id) = rx.recv().await {
                if cancel_token.is_cancelled() {
                    break;
                }
                let _ = GmvDevice::update_gmv_device_status_by_device_id(&device_id, 0).await;
            }
        }

        async fn purge_expired_task(
            shared: Arc<Shared>,
            cancel_token: CancellationToken,
        ) -> GlobalResult<()> {
            loop {
                if cancel_token.is_cancelled() {
                    break;
                }
                if let Some(when) = shared.purge_expired_state().await? {
                    tokio::select! {
                        _ = time::sleep_until(when) =>{},
                        _ = shared.background_task.notified() =>{},
                    }
                } else {
                    shared.background_task.notified().await;
                }
            }
            let mut state = shared.state.lock();
            state.expirations.clear();
            state.sessions.clear();
            state.bill_map.clear();
            Ok(())
        }

        pub fn insert(device_id: &String, heartbeat: u8, bill: &Association) {
            let expires = Duration::from_secs(heartbeat as u64 * 3);
            let when = Instant::now() + expires;

            let mut state = Self::get_ctx().shared.state.lock();

            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, device_id.clone()));
            //当插入时，已有该设备映射时，需删除老数据，插入新数据
            if let Some((when, _expires, old_bill)) = state
                .sessions
                .insert(device_id.clone(), (when, expires, bill.clone()))
            {
                state.expirations.remove(&(when, device_id.clone()));
                state.bill_map.remove(&old_bill);
                state.bill_map.insert(bill.clone(), device_id.clone());
            }
            drop(state);

            if notify {
                Self::get_ctx().shared.background_task.notify_one();
            }
        }

        //用于收到网络出口对端连接断开时，清理rw_session数据
        pub fn clean_rw_session_by_bill(bill: &Association) {
            let mut guard = Self::get_ctx().shared.state.lock();

            let state = &mut *guard;
            state.bill_map.remove(bill).map(|device_id| {
                state
                    .sessions
                    .remove(&device_id)
                    .map(|(when, _expires, _bill)| {
                        state.expirations.remove(&(when, device_id));
                    });
            });
        }

        pub fn get_device_id_by_association(bill: &Association) -> Option<String> {
            let guard = Self::get_ctx().shared.state.lock();
            let res = guard.bill_map.get(bill).map(|device_id| device_id.clone());

            res
        }

        //用于清理rw_session数据及端口TCP网络连接
        pub fn clean_rw_session_and_net(device_id: &String) {
            let res = {
                let mut guard = Self::get_ctx().shared.state.lock();

                let state = &mut *guard;
                if let Some((when, _expires, bill)) = state.sessions.remove(device_id) {
                    state.expirations.remove(&(when, device_id.clone()));
                    state.bill_map.remove(&bill);
                    //通知网络出口关闭TCP连接
                    if &Protocol::TCP == bill.get_protocol() {
                        Some(bill)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(association) = res {
                let _ = Self::get_ctx()
                    .io_tx
                    .try_send(Zip::build_event(Event::new(association, 0)))
                    .hand_log(|msg| warn!("{msg}"));
            }
        }

        pub fn keep_alive(device_id: &String, new_bill: Association) {
            let mut guard = Self::get_ctx().shared.state.lock();

            let state = &mut *guard;
            state
                .sessions
                .get_mut(device_id)
                .map(|(when, expires, bill)| {
                    //UDP的无连接状态，需根据心跳实时刷新其网络三元组
                    if &Protocol::UDP == bill.get_protocol() {
                        state.bill_map.remove(bill);
                        state.bill_map.insert(bill.clone(), device_id.clone());
                        *bill = new_bill;
                    }
                    state.expirations.remove(&(*when, device_id.clone()));
                    let ct = Instant::now() + *expires;
                    *when = ct;
                    state.expirations.insert((ct, device_id.clone()));
                });
        }

        pub fn get_expires_by_device_id(device_id: &String) -> Option<Duration> {
            let guard = Self::get_ctx().shared.state.lock();
            let option_expires = guard
                .sessions
                .get(device_id)
                .map(|(_when, expires, _bill)| *expires);

            option_expires
        }

        pub fn get_association_by_device_id(device_id: &String) -> Option<(Association)> {
            let guard = Self::get_ctx().shared.state.lock();
            let opt = guard
                .sessions
                .get(device_id)
                .map(|(_, _, bill)| (bill.clone()));

            opt
        }

        pub fn has_session_by_device_id(device_id: &String) -> bool {
            let guard = Self::get_ctx().shared.state.lock();

            guard.sessions.contains_key(device_id)
        }
    }

    pub struct SipRequestOutput<'a> {
        pub device_id: &'a String,
        pub association: Association,
        pub request: Request,
    }
    impl<'a> SipRequestOutput<'a> {
        pub fn new(device_id: &'a String, association: Association, request: Request) -> Self {
            Self {
                device_id,
                association,
                request,
            }
        }

        pub async fn send_log(self, log: &str) {
            let cb = default_log_callback(format!("{}:{}", log, self.device_id));
            let _ = self.send(cb).await;
        }

        pub async fn send(self, cb: Callback) -> GlobalResult<()> {
            let sip_pkg = SipPackage::build_request(self.request, self.association, cb);
            RWContext::get_ctx()
                .sip_tx
                .send(sip_pkg)
                .await
                .hand_log(|msg| error!("{msg}"))
        }
    }

    struct Shared {
        state: Mutex<State>,
        background_task: Notify,
    }

    impl Shared {
        //清理过期state,并返回下一个过期瞬间刻度
        async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
            let mut guard = RWContext::get_ctx().shared.state.lock();

            let state = &mut *guard;
            let now = Instant::now();
            while let Some((when, device_id)) = state.expirations.iter().next() {
                if when > &now {
                    return Ok(Some(*when));
                }
                //放入队列中处理，避免阻塞导致锁长期占用:更新DB中设备状态为离线
                let _ = RWContext::get_ctx()
                    .db_task
                    .clone()
                    .try_send(device_id.clone())
                    .hand_log(|msg| warn!("{msg}"));
                //移除会话map
                if let Some((when, _dur, bill)) = state.sessions.remove(device_id) {
                    state.bill_map.remove(&bill);
                    state.expirations.remove(&(when, device_id.to_string()));
                    //通知网络出口关闭TCP连接
                    if Protocol::TCP == bill.protocol {
                        let _ = RWContext::get_ctx()
                            .io_tx
                            .try_send(Zip::build_event(Event::new(bill, 0)))
                            .hand_log(|msg| warn!("{msg}"));
                    }
                }
            }

            Ok(None)
        }
    }

    struct State {
        //映射设备ID，会话发送端，过期瞬时，心跳周期，网络三元组，device_id,msg,dst_addr,time,duration,bill
        sessions: HashMap<String, (Instant, Duration, Association)>,
        //标识设备状态过期时刻，instant,device_id
        expirations: BTreeSet<(Instant, String)>,
        //映射网络三元组与设备ID，bill,device_id
        bill_map: HashMap<Association, String>,
    }

    impl State {
        //获取下一个过期瞬间刻度
        fn next_expiration(&self) -> Option<Instant> {
            self.expirations.first().map(|expiration| expiration.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::Entry;
    use std::collections::{BTreeSet, HashMap};

    #[test]
    fn test_bt_set() {
        let mut set = BTreeSet::new();
        set.insert(2);
        set.insert(1);
        set.insert(6);
        set.insert(3);
        let mut iter = set.iter();
        assert_eq!(Some(&1), iter.next());
        assert_eq!(Some(&2), iter.next());
        assert_eq!(Some(&3), iter.next());
        assert_eq!(Some(&6), iter.next());
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_map_entry() {
        let mut map = HashMap::new();
        map.insert(1, 2);
        map.insert(3, 4);
        map.insert(5, 6);

        match map.entry(3) {
            Entry::Occupied(_) => {
                println!("repeat");
            }
            Entry::Vacant(en) => {
                en.insert(10);
            }
        }
        match map.entry(7) {
            Entry::Occupied(_) => {
                println!("repeat");
            }
            Entry::Vacant(en) => {
                en.insert(8);
            }
        }
        println!("{map:?}");
    }
}
