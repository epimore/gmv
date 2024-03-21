/// 数据读写会话：与网络协议交互
/// UDP：三次心跳超时则移除会话
/// TCP：连接断开或三次心跳超时则移除会话
pub mod rw {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, LockResult, Mutex, MutexGuard};
    use std::thread;
    use std::time::Duration;

    use common::anyhow::{anyhow, Error};
    use common::err::{GlobalResult, TransError};
    use common::err::GlobalError::SysErr;
    use common::log::{error, warn};
    use common::net::shard::{Bill, Event, Protocol, Zip};
    use common::once_cell::sync::Lazy;
    use common::tokio;
    use common::tokio::{task, time};
    use common::tokio::sync::mpsc::{Receiver, Sender};
    use common::tokio::sync::{mpsc, Notify};
    use common::tokio::time::Instant;

    use crate::storage::entity::GmvDevice;
    use crate::storage::mapper;

    static RW_SESSION: Lazy<RWSession> = Lazy::new(|| RWSession::init());

    fn get_rw_session_guard() -> GlobalResult<MutexGuard<'static, State>> {
        let guard = RW_SESSION.shared.state.lock()
            .map_err(|err| SysErr(anyhow!(err.to_string())))
            .hand_err(|msg| error!("{msg}"))?;
        Ok(guard)
    }

    pub struct RWSession {
        shared: Arc<Shared>,
        db_task: Sender<String>,
    }

    impl RWSession {
        fn init() -> Self {
            let (tx, rx) = mpsc::channel(10000);
            let session = RWSession {
                shared: Arc::new(
                    Shared {
                        state: Mutex::new(State { sessions: HashMap::new(), expirations: BTreeSet::new(), bill_map: HashMap::new() }),
                        background_task: Notify::new(),
                    }
                ),
                db_task: tx.clone(),
            };
            let shard = session.shared.clone();
            thread::spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().hand_err(|msg| error!("{msg}")).unwrap();
                rt.spawn(Self::do_update_device_status(rx));
                let _ = rt.block_on(Self::purge_expired_task(shard));
            });
            session
        }
        async fn do_update_device_status(mut rx: Receiver<String>) {
            while let Some(device_id) = rx.recv().await {
                GmvDevice::update_gmv_device_status_by_device_id(&device_id, 0);
            }
        }

        async fn purge_expired_task(shared: Arc<Shared>) -> GlobalResult<()> {
            loop {
                if let Some(when) = shared.purge_expired_state().await? {
                    tokio::select! {
                        _ = time::sleep_until(when) =>{},
                        _ = shared.background_task.notified() =>{},
                    }
                } else {
                    shared.background_task.notified().await;
                }
            }
        }

        pub fn insert(device_id: &String, tx: Sender<Zip>, heartbeat: u8, bill: &Bill) -> GlobalResult<()> {
            let expires = Duration::from_secs(heartbeat as u64 * 3);
            let when = Instant::now() + expires;
            let mut state = get_rw_session_guard()?;
            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, device_id.clone()));
            //当插入时，已有该设备映射时，需删除老数据，插入新数据
            if let Some((_tx, when, _expires, old_bill)) = state.sessions.insert(device_id.clone(), (tx, when, expires, bill.clone())) {
                state.expirations.remove(&(when, device_id.clone()));
                state.bill_map.remove(&old_bill);
                state.bill_map.insert(bill.clone(), device_id.clone());
            }
            drop(state);
            if notify {
                RW_SESSION.shared.background_task.notify_one();
            }
            Ok(())
        }

        //用于收到网络出口对端连接断开时，清理rw_session数据
        pub fn clean_rw_session_by_bill(bill: &Bill) -> GlobalResult<()> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            state.bill_map.remove(bill).map(|device_id| {
                state.sessions.remove(&device_id).map(|(_tx, when, _expires, _bill)| {
                    state.expirations.remove(&(when, device_id));
                });
            });
            Ok(())
        }

        //用于清理rw_session数据及端口TCP网络连接
        //todo 禁用设备时需调用
        pub async fn clean_rw_session_and_net(device_id: &String) -> GlobalResult<()> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            if let Some((tx, when, _expires, bill)) = state.sessions.remove(device_id) {
                state.expirations.remove(&(when, device_id.clone()));
                state.bill_map.remove(&bill);
                //通知网络出口关闭TCP连接
                if &Protocol::TCP == bill.get_protocol() {
                    let _ = tx.send(Zip::build_event(Event::new(bill, 0))).await.hand_err(|msg| warn!("{msg}"));
                }
            }
            Ok(())
        }

        pub fn heart(device_id: &String, new_bill: Bill) -> GlobalResult<()> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            state.sessions.get_mut(device_id).map(|(_tx, when, expires, bill)| {
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
            Ok(())
        }

        pub fn get_bill_by_device_id(device_id: &String) -> GlobalResult<Option<Bill>> {
            let guard = get_rw_session_guard()?;
            let option_bill = guard.sessions.get(device_id).map(|(_tx, _when, _expires, bill)| bill.clone());
            Ok(option_bill)
        }

        pub fn get_expires_by_device_id(device_id: &String) -> GlobalResult<Option<Duration>> {
            let guard = get_rw_session_guard()?;
            let option_expires = guard.sessions.get(device_id).map(|(_tx, _when, expires, _bill)| *expires);
            Ok(option_expires)
        }
    }

    struct Shared {
        state: Mutex<State>,
        background_task: Notify,
    }

    impl Shared {
        //清理过期state,并返回下一个过期瞬间刻度
        async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            let now = Instant::now();
            while let Some((when, device_id)) = state.expirations.iter().next() {
                if when > &now {
                    return Ok(Some(*when));
                }
                //放入队列中处理，避免阻塞导致锁长期占用:更新DB中设备状态为离线
                let _ = RW_SESSION.db_task.clone().send(device_id.clone()).await.hand_err(|msg| warn!("{msg}"));
                // GmvDevice::update_gmv_device_status_by_device_id(device_id, 0);
                //移除会话map
                if let Some((tx, when, _dur, bill)) = state.sessions.remove(device_id) {
                    state.bill_map.remove(&bill);
                    state.expirations.remove(&(when, device_id.to_string()));
                    //通知网络出口关闭TCP连接
                    if &Protocol::TCP == bill.get_protocol() {
                        let _ = tx.send(Zip::build_event(Event::new(bill, 0))).await.hand_err(|msg| warn!("{msg}"));
                    }
                }
            }
            Ok(None)
        }
    }

    struct State {
        //映射设备ID，会话发送端，过期瞬时，心跳周期，网络三元组，device_id,msg,dst_addr,time,duration,bill
        sessions: HashMap<String, (Sender<Zip>, Instant, Duration, Bill)>,
        //标识设备状态过期时刻，instant,device_id
        expirations: BTreeSet<(Instant, String)>,
        //映射网络三元组与设备ID，bill,device_id
        bill_map: HashMap<Bill, String>,
    }

    impl State {
        //获取下一个过期瞬间刻度
        fn next_expiration(&self) -> Option<Instant> {
            self.expirations.iter().next().map(|expiration| expiration.0)
        }
    }
}

/// 事件会话：与业务事件交互
/// 定位：请求 <——> 回复
/// 会话超时 8s
pub mod event{
    pub enum Owner{
        Sys,
        User(String)
    }

    struct State{
        // expirations:
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

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
}

