use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::{RawRwLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{error, info};
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::Notify;
use common::tokio::time;
use common::tokio::time::Instant;
use constructor::Get;

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

pub fn insert(ssrc: u32, stream_id: String, expires: Duration, channel: Channel) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    if !state.sessions.contains_key(&ssrc) {
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc));
        state.sessions.insert(ssrc, (when, stream_id.clone(), expires, channel));
        state.inner.insert(stream_id, ssrc);
        drop(state);
        if notify {
            SESSION.shared.background_task.notify_one();
        }
        Ok(())
    } else { Err(SysErr(anyhow!("ssrc = {:?},媒体流标识重复",ssrc))) }
}

//返回rtp_tx
pub fn refresh(ssrc: u32) -> Option<Sender<Bytes>> {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    state.sessions.get_mut(&ssrc).map(|(when, _, expires, channel)| {
        state.expirations.remove(&(*when, ssrc));
        let ct = Instant::now() + *expires;
        *when = ct;
        state.expirations.insert((ct, ssrc));
        channel.get_rtp_tx()
    })
}

//外层option判断ssrc是否存在，里层判断是否需要rtp/m3u8协议
pub fn get_flv_tx(ssrc: &u32) -> Option<Option<Sender<Bytes>>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_flv_tx())
        }
    }
}

pub fn get_flv_rx(ssrc: &u32) -> Option<Option<Receiver<Bytes>>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_flv_rx())
        }
    }
}

pub fn get_m3u8_tx(ssrc: &u32) -> Option<Option<Sender<Bytes>>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_m3u8_tx())
        }
    }
}

pub fn get_m3u8_rx(ssrc: &u32) -> Option<Option<Receiver<Bytes>>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_m3u8_rx())
        }
    }
}

pub fn get_rtp_tx(ssrc: &u32) -> Option<Sender<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_rtp_tx())
        }
    }
}

pub fn get_rtp_rx(ssrc: &u32) -> Option<Receiver<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel)) => {
            Some(channel.get_rtp_rx())
        }
    }
}


struct Session {
    shared: Arc<Shared>,
}

impl Session {
    fn init() -> Self {
        let session = Session {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    sessions: HashMap::new(),
                    inner: HashMap::new(),
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
}

struct Shared {
    state: RwLock<State>,
    background_task: Notify,
}

impl Shared {
    //清理过期state,并返回下一个过期瞬间刻度
    async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
        let mut guard = SESSION.shared.state.write();
        let state = &mut *guard;
        let now = Instant::now();
        while let Some(&(when, ssrc)) = state.expirations.iter().next() {
            if when > now {
                return Ok(Some(when));
            }
            state.sessions.remove(&ssrc).map(|(_, stream_id, _, _)|
                //todo callback
                state.inner.remove(&stream_id)
            );
            state.expirations.remove(&(when, ssrc));
        }
        Ok(None)
    }
}

///自定义会话信息
struct State {
    sessions: HashMap<u32, (Instant, String, Duration, Channel)>,
    inner: HashMap<String, u32>,
    //(ts,ssrc):stream_id
    expirations: BTreeSet<(Instant, u32)>,
}

impl State {
    //获取下一个过期瞬间刻度
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

pub type InnerChannel = (Sender<Bytes>, Receiver<Bytes>);

#[derive(Debug, Get, Clone)]
pub struct Channel {
    rtp_channel: InnerChannel,
    flv_channel: Option<InnerChannel>,
    m3u8_channel: Option<InnerChannel>,
}

impl Channel {
    pub fn build(rtp_channel: InnerChannel, flv_channel: Option<InnerChannel>, m3u8_channel: Option<InnerChannel>) -> GlobalResult<Self> {
        if flv_channel.is_none() && m3u8_channel.is_none() {
            Err(SysErr(anyhow!("无输出管道。"))).hand_err(|msg| error!("{msg}"))?;
        }
        Ok(Self {
            rtp_channel,
            flv_channel,
            m3u8_channel,
        })
    }
    fn get_rtp_rx(&self) -> Receiver<Bytes> {
        self.rtp_channel.1.clone()
    }
    fn get_rtp_tx(&self) -> Sender<Bytes> {
        self.rtp_channel.0.clone()
    }
    fn get_flv_tx(&self) -> Option<Sender<Bytes>> {
        match self.get_flv_channel() {
            None => { None }
            Some((tx, _rx)) => { Some(tx.clone()) }
        }
    }
    fn get_flv_rx(&self) -> Option<Receiver<Bytes>> {
        match self.get_flv_channel() {
            None => { None }
            Some((_tx, rx)) => { Some(rx.clone()) }
        }
    }
    fn get_m3u8_tx(&self) -> Option<Sender<Bytes>> {
        match self.get_m3u8_channel() {
            None => { None }
            Some((tx, _rx)) => { Some(tx.clone()) }
        }
    }
    fn get_m3u8_rx(&self) -> Option<Receiver<Bytes>> {
        match self.get_m3u8_channel() {
            None => { None }
            Some((_tx, rx)) => { Some(rx.clone()) }
        }
    }
}