/// 数据读写会话：与网络协议交互
/// UDP：三次心跳超时则移除会话
/// TCP：连接断开或三次心跳超时则移除会话
pub mod rw {
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use parking_lot::Mutex;
    use rsip::Request;

    use crate::gb::depot::{default_log_callback, Callback, SipPackage};
    use crate::register::schedule::{ScheduleEvent, TimeScheduler};
    use crate::storage::entity::GmvDevice;

    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::{error, warn};
    use base::net::state::{Association, Event, IoEventType, Protocol, Zip};
    use base::tokio::sync::mpsc;
    use base::tokio::sync::mpsc::{Receiver, Sender};
    use base::tokio_util::sync::CancellationToken;
    use base::utils::rt::GlobalRuntime;

    static RW_CTX: OnceLock<RWContext> = OnceLock::new();

    pub struct RWContext {
        shared: Arc<Shared>,
        time_schedule: TimeScheduler<String>,
        db_task: Sender<String>,
        io_tx: Sender<Zip>,
        sip_tx: Sender<SipPackage>,
    }

    impl RWContext {
        pub fn get_ctx() -> &'static RWContext {
            RW_CTX.get().expect("RWContext not initialized")
        }

        pub fn init(io_tx: Sender<Zip>, sip_tx: Sender<SipPackage>) {
            let (tx, rx) = mpsc::channel(64);
            let session = RWContext {
                shared: Arc::new(Shared {
                    state: Mutex::new(State {
                        sessions: HashMap::new(),
                        bill_map: HashMap::new(),
                    }),
                }),
                time_schedule: TimeScheduler::new(),
                db_task: tx,
                io_tx,
                sip_tx,
            };
            let shared = session.shared.clone();
            let time_schedule = session.time_schedule.clone();
            let rt = GlobalRuntime::get_main_runtime();
            rt.rt_handle
                .spawn(Self::do_update_device_status(rx, rt.cancel.clone()));
            rt.rt_handle
                .spawn(Self::purge_expired_task(shared, time_schedule, rt.cancel));
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
            time_schedule: TimeScheduler<String>,
            cancel_token: CancellationToken,
        ) -> GlobalResult<()> {
            while let Some(batch) = time_schedule.next_batch(&cancel_token).await {
                shared.purge_expired_state(batch).await?;
            }
            let mut state = shared.state.lock();
            state.sessions.clear();
            state.bill_map.clear();
            Ok(())
        }

        pub fn insert(device_id: &String, device_session: DeviceSession) {
            let expires = device_session.expires;
            let association = device_session.association.clone();
            let mut state = Self::get_ctx().shared.state.lock();
            if let Some(old_ds) = state.sessions.insert(device_id.clone(), device_session) {
                state.bill_map.remove(&old_ds.association);
            }
            state.bill_map.insert(association, device_id.clone());
            drop(state);
            let _ = Self::get_ctx()
                .time_schedule
                .insert(device_id.clone(), expires);
        }

        pub fn clean_rw_session_by_bill(bill: &Association) {
            let mut guard = Self::get_ctx().shared.state.lock();
            let state = &mut *guard;
            state.bill_map.remove(bill).map(|device_id| {
                state.sessions.remove(&device_id);
                let _ = Self::get_ctx().time_schedule.remove(&device_id);
            });
        }

        pub fn get_device_id_by_association(bill: &Association) -> Option<String> {
            let guard = Self::get_ctx().shared.state.lock();
            guard.bill_map.get(bill).cloned()
        }

        pub fn clean_rw_session_and_net(device_id: &String) {
            let res = {
                let mut guard = Self::get_ctx().shared.state.lock();
                let state = &mut *guard;
                if let Some(ds) = state.sessions.remove(device_id) {
                    state.bill_map.remove(&ds.association);
                    let _ = Self::get_ctx().time_schedule.remove(device_id);
                    if matches!(ds.association.protocol, Protocol::TCP) {
                        Some(ds.association)
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
                    .try_send(Zip::build_event(Event::new(association, IoEventType::Close)))
                    .hand_log(|msg| warn!("{msg}"));
            }
        }

        pub fn keep_alive(device_id: &String, new_bill: Association) {
            let mut guard = Self::get_ctx().shared.state.lock();
            let state = &mut *guard;
            let should_refresh = state.sessions.get_mut(device_id).map(|ds| {
                state.bill_map.remove(&ds.association);
                state.bill_map.insert(new_bill.clone(), device_id.clone());
                ds.association = new_bill;
            });
            drop(guard);
            if should_refresh.is_some() {
                let _ = Self::get_ctx().time_schedule.refresh(device_id);
            }
        }

        pub fn get_expires_by_device_id(device_id: &String) -> Option<Duration> {
            let guard = Self::get_ctx().shared.state.lock();
            guard.sessions.get(device_id).map(|ds| ds.expires)
        }

        pub fn get_ds_by_device_id(device_id: &String) -> Option<(String, Association, bool)> {
            let guard = Self::get_ctx().shared.state.lock();
            guard.sessions.get(device_id).map(|ds| {
                (
                    ds.contact_uri.clone(),
                    ds.association.clone(),
                    ds.support_lr,
                )
            })
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
    }

    impl Shared {
        async fn purge_expired_state(&self, batch: Vec<ScheduleEvent<String>>) -> GlobalResult<()> {
            let mut guard = RWContext::get_ctx().shared.state.lock();
            let state = &mut *guard;
            for event in batch {
                let device_id = event.key;
                let _ = RWContext::get_ctx()
                    .db_task
                    .clone()
                    .try_send(device_id.clone())
                    .hand_log(|msg| warn!("{msg}"));
                if let Some(ds) = state.sessions.remove(&device_id) {
                    state.bill_map.remove(&ds.association);
                    if Protocol::TCP == ds.association.protocol {
                        let _ = RWContext::get_ctx()
                            .io_tx
                            .try_send(Zip::build_event(Event::new(
                                ds.association,
                                IoEventType::Close,
                            )))
                            .hand_log(|msg| warn!("{msg}"));
                    }
                }
            }
            Ok(())
        }
    }

    pub struct DeviceSession {
        pub contact_uri: String,
        pub association: Association,
        pub support_lr: bool,
        pub expires: Duration,
    }

    impl DeviceSession {
        pub fn build(contact_uri: String, association: Association, heartbeat: u8) -> Self {
            let expires = Duration::from_secs(heartbeat as u64 * 3);
            Self {
                contact_uri,
                association,
                support_lr: false,
                expires,
            }
        }

        pub fn enable_lr(&mut self) {
            self.support_lr = true;
        }
    }

    struct State {
        sessions: HashMap<String, DeviceSession>,
        bill_map: HashMap<Association, String>,
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
