use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{RawRwLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::clap::builder::Str;
use common::err::{BizError, GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{debug, error, info};
use common::net::shared::{Bill, Zip};
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::{broadcast, mpsc, Notify};
use common::tokio::sync::oneshot::Sender;
use common::tokio::time;
use common::tokio::time::Instant;
use constructor::Get;

use crate::biz::call::{BaseStreamInfo, RtpInfo};
use crate::general::mode::{BUFFER_SIZE, ServerConf};
use crate::io::hook_handler;
use crate::io::hook_handler::{Event, EventRes};

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

pub fn insert(ssrc: u32, stream_id: String, expires: Duration, channel: Channel) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    if !state.sessions.contains_key(&ssrc) {
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc));
        state.sessions.insert(ssrc, (when, stream_id.clone(), expires, channel, 0, None));
        state.inner.insert(stream_id, (ssrc, HashSet::new(), HashSet::new()));
        drop(state);
        if notify {
            SESSION.shared.background_task.notify_one();
        }
        Ok(())
    } else { Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC已存在", ssrc), |msg| error!("{msg}"))) }
}

//返回rtp_tx
pub async fn refresh(ssrc: u32, bill: &Bill) -> Option<(crossbeam_channel::Sender<Bytes>, crossbeam_channel::Receiver<Bytes>)> {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    if let Some((when, stream_id, expires, channel, reported_time, info)) = state.sessions.get_mut(&ssrc) {
        state.expirations.remove(&(*when, ssrc));
        let ct = Instant::now() + *expires;
        *when = ct;
        state.expirations.insert((ct, ssrc));
        if *reported_time == 0 {
            let remote_addr_str = bill.get_remote_addr().to_string();
            let protocol_addr = bill.get_protocol().get_value().to_string();
            *info = Some((remote_addr_str.clone(), protocol_addr.clone()));
            let rtp_info = RtpInfo::new(ssrc, protocol_addr, remote_addr_str, SESSION.shared.server_conf.get_name().clone());
            let time = SystemTime::now().duration_since(UNIX_EPOCH).expect("Time went backwards").as_secs() as u32;
            let stream_info = BaseStreamInfo::new(rtp_info, stream_id.clone(), time);
            let _ = SESSION.shared.event_tx.clone().send((Event::streamIn(stream_info), None)).await.hand_err(|msg| error!("{msg}"));
            *reported_time = time;
        }
        return Some((channel.rtp_channel.0.clone(), channel.rtp_channel.1.clone()));
    }
    None
}

//外层option判断ssrc是否存在，里层判断是否需要rtp/hls协议
pub fn get_flv_tx(ssrc: &u32) -> Option<broadcast::Sender<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_flv_tx())
        }
    }
}

pub fn get_flv_rx(ssrc: &u32) -> Option<broadcast::Receiver<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_flv_rx())
        }
    }
}

pub fn get_hls_tx(ssrc: &u32) -> Option<broadcast::Sender<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_hls_tx())
        }
    }
}

pub fn get_hls_rx(ssrc: &u32) -> Option<broadcast::Receiver<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_hls_rx())
        }
    }
}

pub fn get_rtp_tx(ssrc: &u32) -> Option<crossbeam_channel::Sender<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_rtp_tx())
        }
    }
}

pub fn get_rtp_rx(ssrc: &u32) -> Option<crossbeam_channel::Receiver<Bytes>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, channel, _, _)) => {
            Some(channel.get_rtp_rx())
        }
    }
}

pub fn get_server_conf() -> &'static ServerConf {
    let conf = &SESSION.shared.server_conf;
    conf
}

pub fn get_event_tx() -> mpsc::Sender<(Event, Option<Sender<EventRes>>)> {
    SESSION.shared.event_tx.clone()
}

//更新用户数据:in_out:true-插入,false-移除
pub fn update_token(stream_id: &String, play_type: &String, user_token: String, in_out: bool) {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    if let Some((_, flv_sets, hls_sets)) = state.inner.get_mut(stream_id) {
        match &play_type[..] {
            "flv" => { if in_out { flv_sets.insert(user_token); } else { flv_sets.remove(&user_token); } }
            "hls" => { if in_out { hls_sets.insert(user_token); } else { hls_sets.remove(&user_token); } }
            _ => {}
        }
    }
}

//返回BaseStreamInfo,flv_count,hls_count
pub fn get_base_stream_info_by_stream_id(stream_id: &String) -> Option<(BaseStreamInfo, u32, u32)> {
    let guard = SESSION.shared.state.read();
    match guard.inner.get(stream_id) {
        None => {
            None
        }
        Some((ssrc, flv_tokens, hls_tokens)) => {
            match guard.sessions.get(ssrc) {
                Some((ts, stream_id, dur, ch, stream_in_reported_time, Some((origin_addr, protocol)))) => {
                    let server_name = SESSION.shared.server_conf.get_name().to_string();
                    let rtp_info = RtpInfo::new(*ssrc, protocol.to_string(), origin_addr.to_string(), server_name);
                    let stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *stream_in_reported_time);
                    Some((stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32))
                }
                _ => { None }
            }
        }
    }
}


struct Session {
    shared: Arc<Shared>,
}

impl Session {
    fn init() -> Self {
        let tripe = common::init();
        ffmpeg_next::init().expect("ffmpeg init failed");
        let cfg_yaml = tripe.get_cfg().get(0).clone().expect("config file is invalid");
        let server_conf = ServerConf::build(cfg_yaml);
        banner();
        let (tx, rx) = mpsc::channel(1000);
        let session = Session {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    sessions: HashMap::new(),
                    inner: HashMap::new(),
                    expirations: BTreeSet::new(),
                }),
                background_task: Notify::new(),
                server_conf: server_conf.clone(),
                event_tx: tx,
            })
        };
        let shared = session.shared.clone();
        thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().thread_name("SESSION").build().hand_err(|msg| error!("{msg}")).unwrap();

            let _ = rt.block_on(Self::purge_expired_task(shared));
        });
        thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().thread_name("HOOK-EVENT").build().hand_err(|msg| error!("{msg}")).unwrap();
            let _ = rt.block_on(Event::event_loop(rx));
        });
        println!("Server node name = {}\n\
        Listen to http api addr = 0.0.0.0:{}\n\
        Listen to rtp over tcp and udp,stream addr = 0.0.0.0:{}\n\
        Listen to rtcp over tcp and udp,message addr = 0.0.0.0:{}\n\
        Hook to http addr = {}\n\
        ... GMV:STREAM started.",
                 server_conf.get_name(),
                 server_conf.get_http_port(),
                 server_conf.get_rtp_port(),
                 server_conf.get_rtcp_port(),
                 server_conf.get_hook_uri());
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
    server_conf: ServerConf,
    event_tx: mpsc::Sender<(Event, Option<Sender<EventRes>>)>,
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
            state.sessions.remove(&ssrc).map(|(_, stream_id, _, _, _, _)|
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
    //ssrc,(ts,stream_id,dur,ch,stream_in_reported_time,(origin_addr,protocol))
    sessions: HashMap<u32, (Instant, String, Duration, Channel, u32, Option<(String, String)>)>,
    //stream_id:(ssrc,flv-tokens,hls-tokens)
    inner: HashMap<String, (u32, HashSet<String>, HashSet<String>)>,
    //(ts,ssrc)
    expirations: BTreeSet<(Instant, u32)>,
}

impl State {
    //获取下一个过期瞬间刻度
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

type SyncChannel = (crossbeam_channel::Sender<Bytes>, crossbeam_channel::Receiver<Bytes>);
type BroadcastChannel = (broadcast::Sender<Bytes>, broadcast::Receiver<Bytes>);

#[derive(Debug)]
pub struct Channel {
    rtp_channel: SyncChannel,
    flv_channel: BroadcastChannel,
    hls_channel: BroadcastChannel,
}

impl Channel {
    pub fn build() -> Self {
        let rtp_channel = crossbeam_channel::bounded(BUFFER_SIZE);
        let flv_channel = broadcast::channel(BUFFER_SIZE);
        let hls_channel = broadcast::channel(BUFFER_SIZE);
        Self {
            rtp_channel,
            flv_channel,
            hls_channel,
        }
    }
    fn get_rtp_rx(&self) -> crossbeam_channel::Receiver<Bytes> {
        self.rtp_channel.1.clone()
    }
    fn get_rtp_tx(&self) -> crossbeam_channel::Sender<Bytes> {
        self.rtp_channel.0.clone()
    }
    fn get_flv_tx(&self) -> broadcast::Sender<Bytes> {
        self.flv_channel.0.clone()
    }
    fn get_flv_rx(&self) -> broadcast::Receiver<Bytes> {
        self.flv_channel.0.subscribe()
    }
    fn get_hls_tx(&self) -> broadcast::Sender<Bytes> {
        self.hls_channel.0.clone()
    }
    fn get_hls_rx(&self) -> broadcast::Receiver<Bytes> {
        self.hls_channel.0.subscribe()
    }
}

fn banner() {
    let br = r#"
            ___   __  __  __   __    _      ___    _____    ___     ___     ___   __  __
    o O O  / __| |  \/  | \ \ / /   (_)    / __|  |_   _|  | _ \   | __|   /   \ |  \/  |
   o      | (_ | | |\/| |  \ V /     _     \__ \    | |    |   /   | _|    | - | | |\/| |
  oO__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   _|_|_   |_|_\   |___|   |_|_| |_|__|_|
 {======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""T""|_|""R""|_|""E""|_|""A""|_|""M""|
./o--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
"#;
    println!("{}", br);
}