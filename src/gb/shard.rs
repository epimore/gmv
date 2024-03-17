pub mod rw {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, LockResult, Mutex, MutexGuard};
    use std::thread;
    use std::time::Duration;

    use common::anyhow::{anyhow, Error};
    use common::err::{GlobalResult, TransError};
    use common::err::GlobalError::SysErr;
    use common::log::{error, warn};
    use common::net::shard::Zip;
    use common::once_cell::sync::Lazy;
    use common::tokio;
    use common::tokio::{task, time};
    use common::tokio::sync::mpsc::Sender;
    use common::tokio::sync::Notify;
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
    }

    impl RWSession {
        fn init() -> Self {
            let session = RWSession {
                shared: Arc::new(
                    Shared {
                        state: Mutex::new(State { sessions: HashMap::new(), expirations: BTreeSet::new() }),
                        background_task: Notify::new(),
                    }
                )
            };
            let shard = session.shared.clone();
            thread::spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().hand_err(|msg| error!("{msg}")).unwrap();
                let _ = rt.block_on(Self::purge_expired_task(shard));
            });
            session
        }

        async fn purge_expired_task(shared: Arc<Shared>) -> GlobalResult<()> {
            loop {
                if let Some(when) = shared.purge_expired_state()? {
                    tokio::select! {
                        _ = time::sleep_until(when) =>{},
                        _ = shared.background_task.notified() =>{},
                    }
                } else {
                    shared.background_task.notified().await;
                }
            }
        }

        pub fn insert(device_id: &String, tx: Sender<Zip>, heartbeat: u8) -> GlobalResult<()> {
            let expires = Duration::from_secs(heartbeat as u64);
            let when = Instant::now() + expires;
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, device_id.clone()));
            let pre = state.sessions.insert(device_id.clone(), (tx, when, expires));
            if let Some((_tx, when, _expires)) = pre {
                state.expirations.remove(&(when, device_id.clone()));
            }
            drop(state);
            if notify {
                RW_SESSION.shared.background_task.notify_one();
            }
            Ok(())
        }

        pub fn clean(device_id: &String) -> GlobalResult<()> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            state.sessions.remove(device_id).map(|(_tx, when, _expires)| {
                state.expirations.remove(&(when, device_id.clone()));
            });
            Ok(())
        }

        pub fn heart(device_id: &String) -> GlobalResult<()> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            state.sessions.get_mut(device_id).map(|(_tx, when, expires)| {
                state.expirations.remove(&(*when, device_id.clone()));
                let ct = Instant::now() + *expires;
                *when = ct;
                state.expirations.insert((ct, device_id.clone()));
            });
            Ok(())
        }
    }

    struct Shared {
        state: Mutex<State>,
        background_task: Notify,
    }

    impl Shared {
        //清理过期state,并返回下一个过期瞬间刻度
        fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
            let mut guard = get_rw_session_guard()?;
            let state = &mut *guard;
            let now = Instant::now();
            while let Some((when, device_id)) = state.expirations.iter().next() {
                if when > &now {
                    return Ok(Some(*when));
                }
                //todo 放入队列中处理，避免阻塞导致锁长期占用
                GmvDevice::update_gmv_device_status_by_device_id(device_id, 0);
                state.sessions.remove(device_id);
                state.expirations.remove(&(*when, device_id.to_string()));
            }
            Ok(None)
        }
    }

    struct State {
        //device_id,msg,dst_addr,time,duration,0-UDP/1-TCP
        pub sessions: HashMap<String, (Sender<Zip>, Instant, Duration)>,
        pub expirations: BTreeSet<(Instant, String)>,
    }

    impl State {
        //获取下一个过期瞬间刻度
        fn next_expiration(&self) -> Option<Instant> {
            self.expirations.iter().next().map(|expiration| expiration.0)
        }
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

