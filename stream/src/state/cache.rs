use std::collections::{BTreeSet, HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use parking_lot::RwLock;

use rtp::packet::Packet;

use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::error;
use common::net::state::Association;
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::{broadcast, mpsc, Notify};
use common::tokio::sync::oneshot::Sender;
use common::tokio::time;
use common::tokio::time::Instant;

use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamState};
use crate::coder::FrameData;
use crate::general::mode::{BUFFER_SIZE, HALF_TIME_OUT, Media, ServerConf};
use crate::io::hook_handler::{Event, EventRes};

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

pub fn insert_media_type(ssrc: u32, media_type: HashMap<u8, Media>) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    match state.sessions.entry(ssrc) {
        Entry::Occupied(mut occ) => {
            let (.., mt) = occ.get_mut();
            *mt = media_type;
            Ok(())
        }
        Entry::Vacant(_) => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
    }
}

pub fn get_rx_media_type(ssrc: &u32) -> Option<(crossbeam_channel::Receiver<Packet>, HashMap<u8, Media>)> {
    let state = SESSION.shared.state.read();
    state.sessions.get(ssrc).map(|mt| (mt.4.get_rtp_rx(), mt.7.clone()))
}

pub fn insert(ssrc: u32, stream_id: String, channel: Channel) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    if !state.sessions.contains_key(&ssrc) {
        let expires = Duration::from_millis(HALF_TIME_OUT);
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc));
        state.sessions.insert(ssrc, (AtomicBool::new(true), when, stream_id.clone(), expires, channel, 0, None, HashMap::new()));
        state.inner.insert(stream_id, (ssrc, HashSet::new(), HashSet::new(), None));
        drop(state);
        if notify {
            SESSION.shared.background_task.notify_one();
        }
        Ok(())
    } else { Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC已存在", ssrc), |msg| error!("{msg}"))) }
}

//返回rtp_tx
pub fn refresh(ssrc: u32, bill: &Association) -> Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)> {
    let guard = SESSION.shared.state.read();
    if let Some((on, _when, stream_id, _expires, channel, reported_time, _info, _media)) = guard.sessions.get(&ssrc) {
        if let Some((_ssrc, _flv_sets, _hls_sets, _record)) = guard.inner.get(stream_id) {
            //更新流状态：时间轮会扫描流，将其置为false，若使用中则on更改为true;
            //增加判断流是否使用,若使用则更新流状态;目的：流空闲则断流。
            // if flv_sets.len() > 0 || hls_sets.len() > 0 || record.is_some() {
            if !on.load(Ordering::SeqCst) {
                on.store(true, Ordering::SeqCst);
            }
            // }
        }
        return if reported_time == &0 {
            drop(guard);
            //回调流注册时-事件
            event_stream_in(ssrc, bill)
        } else {
            Some((channel.rtp_channel.0.clone(), channel.rtp_channel.1.clone()))
        }
    }
    None
}

fn event_stream_in(ssrc: u32, bill: &Association) ->Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)>{
    let mut guard = SESSION.shared.state.write();
    if let Some((_on, _when, stream_id, _expires, channel, reported_time, info, _)) = guard.sessions.get_mut(&ssrc) {
        let remote_addr_str = bill.get_remote_addr().to_string();
        let protocol_addr = bill.get_protocol().get_value().to_string();
        *info = Some((remote_addr_str.clone(), protocol_addr.clone()));
        let rtp_info = RtpInfo::new(ssrc, Some(protocol_addr), Some(remote_addr_str), SESSION.shared.server_conf.get_name().clone());
        let time = SystemTime::now().duration_since(UNIX_EPOCH).expect("Time went backwards").as_secs() as u32;
        let stream_info = BaseStreamInfo::new(rtp_info, stream_id.clone(), time);
        let _ = SESSION.shared.event_tx.clone().try_send((Event::StreamIn(stream_info), None)).hand_log(|msg| error!("{msg}"));
        *reported_time = time;
        return Some((channel.rtp_channel.0.clone(), channel.rtp_channel.1.clone()));
    }
    return None;
}

//外层option判断ssrc是否存在，里层判断是否需要rtp/hls协议
pub fn get_flv_tx(ssrc: &u32) -> Option<broadcast::Sender<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
            Some(channel.get_flv_tx())
        }
    }
}

pub fn get_flv_rx(ssrc: &u32) -> Option<broadcast::Receiver<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
            Some(channel.get_flv_rx())
        }
    }
}

pub fn get_hls_tx(ssrc: &u32) -> Option<broadcast::Sender<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
            Some(channel.get_hls_tx())
        }
    }
}

pub fn get_hls_rx(ssrc: &u32) -> Option<broadcast::Receiver<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
            Some(channel.get_hls_rx())
        }
    }
}

pub fn get_rtp_tx(ssrc: &u32) -> Option<crossbeam_channel::Sender<Packet>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
            Some(channel.get_rtp_tx())
        }
    }
}

pub fn get_rtp_rx(ssrc: &u32) -> Option<crossbeam_channel::Receiver<Packet>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some((_, _, _, _, channel, _, _, _)) => {
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
    if let Some((_, flv_sets, hls_sets, _)) = state.inner.get_mut(stream_id) {
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
        Some((ssrc, flv_tokens, hls_tokens, _)) => {
            match guard.sessions.get(ssrc) {
                Some((_, _ts, stream_id, _dur, _ch, stream_in_reported_time, Some((origin_addr, protocol)), _)) => {
                    let server_name = SESSION.shared.server_conf.get_name().to_string();
                    let rtp_info = RtpInfo::new(*ssrc, Some(protocol.to_string()), Some(origin_addr.to_string()), server_name);
                    let stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *stream_in_reported_time);
                    Some((stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32))
                }
                _ => { None }
            }
        }
    }
}

pub fn remove_by_stream_id(stream_id: &String) {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    if let Some((ssrc, _, _, _)) = state.inner.remove(stream_id) {
        if let Some((_, when, _, _, _, _, _, _)) = state.sessions.remove(&ssrc) {
            state.expirations.remove(&(when, ssrc));
        }
    }
}

pub fn get_stream_state(opt_stream_id: Option<String>) -> Vec<StreamState> {
    let mut vec = Vec::new();
    let guard = SESSION.shared.state.read();
    let server_name = SESSION.shared.server_conf.get_name().to_string();
    match opt_stream_id {
        None => {
            //ssrc,(on,ts,stream_id,dur,ch,stream_in_reported_time,(origin_addr,protocol))
            for (ssrc, (_, _, stream_id, _, _, report_timestamp, opt_addr_protocol, _)) in &guard.sessions {
                let mut origin_addr = None;
                let mut protocol = None;
                if let Some((addr, proto)) = opt_addr_protocol {
                    origin_addr = Some(addr.clone());
                    protocol = Some(proto.clone());
                }
                let rtp_info = RtpInfo::new(*ssrc, protocol, origin_addr, server_name.clone());
                let base_stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *report_timestamp);
                //stream_id:(ssrc,flv-tokens,hls-tokens,record_name)
                if let Some((_ssrc, flv_tokens, hls_tokens, record_name)) = guard.inner.get(stream_id) {
                    let state = StreamState::new(base_stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32, record_name.clone());
                    vec.push(state);
                }
            }
        }
        Some(stream_id) => {
            if let Some((ssrc, flv_tokens, hls_tokens, record_name)) = &guard.inner.get(&stream_id) {
                if let Some((_, _, stream_id, _, _, report_timestamp, opt_addr_protocol, _)) = &guard.sessions.get(ssrc) {
                    let mut origin_addr = None;
                    let mut protocol = None;
                    if let Some((addr, proto)) = opt_addr_protocol {
                        origin_addr = Some(addr.clone());
                        protocol = Some(proto.clone());
                    }
                    let rtp_info = RtpInfo::new(*ssrc, protocol, origin_addr, server_name.clone());
                    let base_stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *report_timestamp);
                    let state = StreamState::new(base_stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32, record_name.clone());
                    vec.push(state);
                }
            }
        }
    }
    vec
}


struct Session {
    shared: Arc<Shared>,
}

impl Session {
    fn init() -> Self {
        let server_conf = ServerConf::init_by_conf();
        let (tx, rx) = mpsc::channel(10000);
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
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().thread_name("SESSION").build().hand_log(|msg| error!("{msg}")).unwrap();
            let _ = rt.block_on(Self::purge_expired_task(shared));
        });
        thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().thread_name("HOOK-EVENT").build().hand_log(|msg| error!("{msg}")).unwrap();
            let _ = rt.block_on(Event::event_loop(rx));
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
    server_conf: ServerConf,
    event_tx: mpsc::Sender<(Event, Option<Sender<EventRes>>)>,
}

impl Shared {
    //清理过期state,并返回下一个过期瞬间刻度
    //判断是否有数据:
    // 有:on=true变false;重新插入计时队列，更新时刻
    // 无：on->false；清理state，并回调通知timeout
    async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
        let mut guard = SESSION.shared.state.write();
        let state = &mut *guard;
        let now = Instant::now();
        let mut notify_one = false;
        while let Some(&(when, ssrc)) = state.expirations.iter().next() {
            if when > now {
                return Ok(Some(when));
            }
            state.expirations.remove(&(when, ssrc));
            let mut del = false;
            if let Some((on, ts, _, _, _, _, _, _)) = state.sessions.get_mut(&ssrc) {
                if on.load(Ordering::SeqCst) {
                    on.store(false, Ordering::SeqCst);
                    let expires = Duration::from_millis(HALF_TIME_OUT);
                    let when = Instant::now() + expires;
                    *ts = when;
                    let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
                    state.expirations.insert((when, ssrc));
                    if notify {
                        notify_one = notify;
                    }
                } else {
                    del = true;
                }
            }
            if del {
                if let Some((_, _, stream_id, _, _, stream_in_reported_time, op, _)) = state.sessions.remove(&ssrc) {
                    if let Some((ssrc, flv_tokens, hls_tokens, record_name)) = state.inner.remove(&stream_id) {
                        //callback stream timeout
                        let server_name = SESSION.shared.server_conf.get_name().to_string();
                        let mut origin_addr = None;
                        let mut protocol = None;
                        if let Some((origin_addr_s, protocol_s)) = op {
                            origin_addr = Some(origin_addr_s);
                            protocol = Some(protocol_s);
                        }
                        let rtp_info = RtpInfo::new(ssrc, protocol, origin_addr, server_name);
                        let stream_info = BaseStreamInfo::new(rtp_info, stream_id, stream_in_reported_time);
                        let stream_state = StreamState::new(stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32, record_name);
                        let _ = SESSION.shared.event_tx.clone().send((Event::StreamTimeout(stream_state), None)).await.hand_log(|msg| error!("{msg}"));
                    }
                }
            }
        }
        if notify_one {
            SESSION.shared.background_task.notify_one();
        }
        Ok(None)
    }
}

///自定义会话信息
struct State {
    //ssrc,(on,ts,stream_id,dur,ch,stream_in_reported_time,(origin_addr,protocol),(media_type,media_type_enum))
    sessions: HashMap<u32, (AtomicBool, Instant, String, Duration, Channel, u32, Option<(String, String)>, HashMap<u8, Media>)>,
    //stream_id:(ssrc,flv-tokens,hls-tokens,record_name)
    inner: HashMap<String, (u32, HashSet<String>, HashSet<String>, Option<String>)>,
    //(ts,ssrc)
    expirations: BTreeSet<(Instant, u32)>,
}

impl State {
    //获取下一个过期瞬间刻度
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

type CrossbeamChannel = (crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>);
type BroadcastChannel = (broadcast::Sender<FrameData>, broadcast::Receiver<FrameData>);

#[derive(Debug)]
pub struct Channel {
    rtp_channel: CrossbeamChannel,
    flv_channel: BroadcastChannel,
    hls_channel: BroadcastChannel,
}

impl Channel {
    pub fn build() -> Self {
        let rtp_channel = crossbeam_channel::bounded(BUFFER_SIZE * 10);
        let flv_channel = broadcast::channel(BUFFER_SIZE);
        let hls_channel = broadcast::channel(BUFFER_SIZE);
        Self {
            rtp_channel,
            flv_channel,
            hls_channel,
        }
    }
    fn get_rtp_rx(&self) -> crossbeam_channel::Receiver<Packet> {
        self.rtp_channel.1.clone()
    }
    fn get_rtp_tx(&self) -> crossbeam_channel::Sender<Packet> {
        self.rtp_channel.0.clone()
    }
    fn get_flv_tx(&self) -> broadcast::Sender<FrameData> {
        self.flv_channel.0.clone()
    }
    fn get_flv_rx(&self) -> broadcast::Receiver<FrameData> {
        self.flv_channel.0.subscribe()
    }
    fn get_hls_tx(&self) -> broadcast::Sender<FrameData> {
        self.hls_channel.0.clone()
    }
    fn get_hls_rx(&self) -> broadcast::Receiver<FrameData> {
        self.hls_channel.0.subscribe()
    }
}