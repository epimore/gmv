use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use common::anyhow::anyhow;
use common::err::GlobalError::SysErr;
use common::err::{GlobalResult, TransError};
use common::log::{error, info};
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::Notify;
use common::tokio::time;
use common::tokio::time::Instant;
use crate::data::buffer::Cache;

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

fn get_session_guard() -> GlobalResult<MutexGuard<'static, State>> {
    let guard = SESSION.shared.state.lock()
        .map_err(|err| SysErr(anyhow!(err.to_string())))
        .hand_err(|msg| error!("{msg}"))?;
    Ok(guard)
}

struct Session {
    shared: Arc<Shared>,
}

impl Session {
    fn init() -> Self {
        let session = Session {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    sessions: HashMap::new(),
                    expirations: BTreeSet::new(),
                }),
                background_task: Notify::new(),
            })
        };
        let shared = session.shared.clone();
        thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().thread_name("SESSION").build().hand_err(|msg| error!("{msg}")).unwrap();
            let _ = rt.block_on(Self::purge_expired_task(shared));
        });
        session
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

    pub fn insert(ssrc: u32, stream_id: String, expires: Duration, ext: Option<Ext>) -> GlobalResult<()> {
        let mut state = get_session_guard()?;
        match state.sessions.entry(ssrc) {
            Entry::Occupied(_) => { Err(SysErr(anyhow!("ssrc = {:?},媒体流标识重复",ssrc))) }
            Entry::Vacant(en) => {
                Cache::add_ssrc(ssrc)?;
                let when = Instant::now() + expires;
                let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
                state.expirations.insert((when, ssrc));
                state.sessions.insert(ssrc, (when, stream_id, expires, ext));
                drop(state);
                if notify {
                    SESSION.shared.background_task.notify_one();
                }
                Ok(())
            }
        }
    }

    pub fn refresh(ssrc: u32) -> GlobalResult<()> {
        let mut guard = get_session_guard()?;
        let state = &mut *guard;
        state.sessions.get_mut(&ssrc).map(|(when, _, expires, _)| {
            state.expirations.remove(&(*when, ssrc));
            let ct = Instant::now() + *expires;
            *when = ct;
            state.expirations.insert((ct, ssrc));
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
    async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
        let mut guard = get_session_guard()?;
        let state = &mut *guard;
        let now = Instant::now();
        while let Some(&(when, ssrc)) = state.expirations.iter().next() {
            if when > now {
                return Ok(Some(when));
            }
            Cache::rm_ssrc(&ssrc);
            state.sessions.remove(&ssrc).map(|(obj)| info!("todo callback {obj:?}"));
            state.expirations.remove(&(when, ssrc));
        }
        Ok(None)
    }
}

///自定义会话信息
struct State {
    sessions: HashMap<u32, (Instant, String, Duration, Option<Ext>)>,
    //(ts,ssrc):stream_id
    expirations: BTreeSet<(Instant, u32)>,
}

impl State {
    //获取下一个过期瞬间刻度
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

#[derive(Debug)]
struct Ext {}