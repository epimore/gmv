use std::collections::{BTreeSet, HashMap};
use std::collections::hash_map::Entry;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::chrono::{Local, Timelike};
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::{error, info};
use common::net::state::Association;
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::{broadcast, mpsc, Mutex, Notify};
use common::tokio::sync::oneshot::Sender;
use common::tokio::time;
use common::tokio::time::Instant;
use parking_lot::RwLock;
use rtp::packet::Packet;

use crate::biz::api::{HlsPiece, SsrcLisDto};
use crate::biz::call::{BaseStreamInfo, NetSource, RtpInfo, StreamState};
use crate::coder::FrameData;
use crate::container::PlayType;
use crate::general::cfg;
use crate::general::mode::{BUFFER_SIZE, HALF_TIME_OUT, Media, ServerConf};
use crate::io::hook_handler::{InEvent, OutEvent, OutEventRes};

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

pub fn insert_media_type(ssrc: u32, media_type: HashMap<u8, Media>) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    match state.sessions.entry(ssrc) {
        Entry::Occupied(mut occ) => {
            let stream_trace = occ.get_mut();
            stream_trace.media_map = media_type;
            if let Err(_error) = stream_trace.event_channel.0.send(InEvent::MediaInit()) {
                return Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},事件接收通道销毁", ssrc), |msg| error!("{msg}")));
            }
            Ok(())
        }
        Entry::Vacant(_) => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
    }
}

pub fn get_in_event_shard_rx(ssrc: &u32) -> Option<Arc<Mutex<broadcast::Receiver<InEvent>>>> {
    let state = SESSION.shared.state.read();
    state.sessions.get(ssrc).map(|st| {
        st.event_channel.1.clone()
    })
}

pub fn get_in_event_sub_rx(ssrc: &u32) -> Option<broadcast::Receiver<InEvent>> {
    let state = SESSION.shared.state.read();
    state.sessions.get(ssrc).map(|st| {
        st.event_channel.0.subscribe()
    })
}

pub fn get_rx_media_type(ssrc: &u32) -> Option<(Media, HalfChannel)> {
    let state = SESSION.shared.state.read();
    if let Some(mt) = state.sessions.get(ssrc) {
        if let Some(media) = mt.media_map.get(&mt.rtp_payload_type) {
            let half_channel = HalfChannel {
                rtp_rx: mt.stream_ch.get_rtp_rx(),
                flv_tx: mt.stream_ch.get_flv_tx(),
                hls_tx: mt.stream_ch.get_hls_tx(),
            };
            return Some((*media, half_channel));
        }
        error!("ssrc: {}, Media payload type: {} is invalid or unsupported",ssrc,&mt.rtp_payload_type);
    }
    error!("ssrc: {} is invalid",ssrc);
    None
}

fn build_out_expires(expires: i32) -> Option<Duration> {
    match expires {
        0 => {
            Some(Duration::default())
        }
        a if a > 0 => {
            Some(Duration::from_secs(a as u64))
        }
        _ => {
            None
        }
    }
}

pub fn insert(ssrc_lis: SsrcLisDto) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    let ssrc = ssrc_lis.ssrc;
    if !state.sessions.contains_key(&ssrc) {
        let expires = Duration::from_millis(HALF_TIME_OUT);
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc, StreamDirection::StreamIn));
        let stream_id = ssrc_lis.stream_id;
        let stream_conf = cfg::StreamConf::init_by_conf()?;
        let out_expires: &i32 = stream_conf.get_expires();
        let out_expires = match ssrc_lis.expires {
            None => {
                build_out_expires(*out_expires)
            }
            Some(val) => {
                build_out_expires(val)
            }
        };
        let (tx, rx) = broadcast::channel(BUFFER_SIZE);
        let stream_trace = StreamTrace {
            stream_id: stream_id.clone(),
            in_on: AtomicBool::new(true),
            in_timeout: when,
            in_expires: expires,
            out_expires,
            stream_ch: Channel::build(),
            register_ts: 0,
            origin_trans: None,
            media_map: Default::default(),
            rtp_payload_type: 0,
            event_channel: (tx, Arc::new(Mutex::new(rx))),
            flv: false,
            hls: None,
        };
        state.sessions.insert(ssrc, stream_trace);
        let inner = InnerTrace { ssrc, user_map: Default::default() };
        state.inner.insert(stream_id, inner);
        drop(state);
        if notify {
            SESSION.shared.background_task.notify_one();
        }
        Ok(())
    } else { Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC已存在", ssrc), |msg| error!("{msg}"))) }
}

//返回rtp_tx
pub fn refresh(ssrc: u32, bill: &Association, packet: &Packet) -> Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)> {
    let guard = SESSION.shared.state.read();
    if let Some(stream_trace) = guard.sessions.get(&ssrc) {
        if !stream_trace.in_on.load(Ordering::SeqCst) {
            stream_trace.in_on.store(true, Ordering::SeqCst);
        }
        //流首次注册
        return if stream_trace.rtp_payload_type == 0 {
            drop(guard);
            //回调流注册时-事件
            let media_payload_type = packet.header.payload_type;
            event_stream_in(ssrc, bill, media_payload_type)
        } else {
            Some((stream_trace.stream_ch.rtp_channel.0.clone(), stream_trace.stream_ch.rtp_channel.1.clone()))
        };
    }
    None
}

pub fn get_stream_id(ssrc: &u32) -> Option<String> {
    let guard = SESSION.shared.state.read();
    guard.sessions.get(ssrc).map(|val| val.stream_id.clone())
}

fn event_stream_in(ssrc: u32, bill: &Association, media_payload_type: u8) -> Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)> {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    let next_expiration = state.next_expiration();
    if let Some(stream_trace) = state.sessions.get_mut(&ssrc) {
        stream_trace.rtp_payload_type = media_payload_type;
        //首次流闲置超时，非永不超时则-默认6秒
        if let Some(mut expires) = stream_trace.out_expires {
            if expires == Duration::default() { expires = Duration::from_secs(6); }
            let when = Instant::now() + expires;
            let notify = next_expiration.map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, ssrc, StreamDirection::StreamOut));
            if notify {
                SESSION.shared.background_task.notify_one();
            }
        }
        let remote_addr_str = bill.get_remote_addr().to_string();
        let protocol = bill.get_protocol().get_value().to_string();
        stream_trace.origin_trans = Some((remote_addr_str.clone(), protocol.clone()));
        let net_source = NetSource::new(remote_addr_str, protocol);
        let rtp_info = RtpInfo::new(ssrc, Some(net_source), SESSION.shared.server_conf.get_name().clone());
        let time = SystemTime::now().duration_since(UNIX_EPOCH).expect("Time went backwards").as_secs() as u32;
        let stream_info = BaseStreamInfo::new(rtp_info, stream_trace.stream_id.clone(), time);
        let _ = SESSION.shared.event_tx.clone().try_send((OutEvent::StreamIn(stream_info), None)).hand_log(|msg| error!("{msg}"));
        stream_trace.register_ts = time;
        if let Err(_err) = stream_trace.event_channel.0.send(InEvent::StreamIn()) {
            error!("ssrc:{}, 事件接收端drop",ssrc);
        }
        return Some((stream_trace.stream_ch.get_rtp_tx(), stream_trace.stream_ch.get_rtp_rx()));
    }
    None
}


pub fn get_flv_tx(ssrc: &u32) -> Option<broadcast::Sender<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some(stream_trace) => {
            Some(stream_trace.stream_ch.get_flv_tx())
        }
    }
}

pub fn get_flv_rx(ssrc: &u32) -> Option<broadcast::Receiver<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some(stream_trace) => {
            Some(stream_trace.stream_ch.get_flv_rx())
        }
    }
}

pub fn get_hls_tx(ssrc: &u32) -> Option<broadcast::Sender<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some(stream_trace) => {
            Some(stream_trace.stream_ch.get_hls_tx())
        }
    }
}

pub fn get_hls_rx(ssrc: &u32) -> Option<broadcast::Receiver<FrameData>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => { None }
        Some(stream_trace) => {
            Some(stream_trace.stream_ch.get_hls_rx())
        }
    }
}

pub fn get_server_conf() -> &'static ServerConf {
    let conf = &SESSION.shared.server_conf;
    conf
}

pub fn get_event_tx() -> mpsc::Sender<(OutEvent, Option<Sender<OutEventRes>>)> {
    SESSION.shared.event_tx.clone()
}

//更新用户数据:in_out:true-插入,false-移除
pub fn update_token(stream_id: &String, play_type: PlayType, user_token: String, in_out: bool, remote_addr: SocketAddr) {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    if let Some(InnerTrace { user_map, ssrc }) = state.inner.get_mut(stream_id) {
        match in_out {
            true => {
                let user = UserTrace {
                    token: user_token,
                    request_time: Local::now().second(),
                    play_type,
                };
                user_map.insert(remote_addr, user);
            }
            false => {
                user_map.remove(&remote_addr);
                if user_map.len() == 0 {
                    if let Some(StreamTrace { out_expires, .. }) = state.sessions.get(ssrc) {
                        if let Some(timeout) = out_expires {
                            let when = Instant::now() + *timeout;
                            state.expirations.insert((when, *ssrc, StreamDirection::StreamOut));
                            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
                            if notify {
                                SESSION.shared.background_task.notify_one();
                            }
                        }
                    }
                }
            }
        }
    }
}

//返回BaseStreamInfo,user_count
pub fn get_base_stream_info_by_stream_id(stream_id: &String) -> Option<(BaseStreamInfo, u32)> {
    let guard = SESSION.shared.state.read();
    if let Some(InnerTrace { ssrc, user_map }) = guard.inner.get(stream_id) {
        if let Some(stream_trace) = guard.sessions.get(ssrc) {
            if let Some((protocol, origin_addr)) = &stream_trace.origin_trans {
                let stream_in_reported_time = stream_trace.register_ts;
                let server_name = SESSION.shared.server_conf.get_name().to_string();
                let net_source = NetSource::new(origin_addr.to_string(), protocol.to_string());
                let rtp_info = RtpInfo::new(*ssrc, Some(net_source), server_name);
                let stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), stream_in_reported_time);
                return Some((stream_info, user_map.len() as u32));
            }
        }
    }
    None
}

pub fn get_stream_count(opt_stream_id: Option<&String>) -> u32 {
    let guard = SESSION.shared.state.read();
    match opt_stream_id {
        None => {
            let len = guard.inner.len();
            len as u32
        }
        Some(stream_id) => {
            if guard.inner.get(stream_id).is_none() { 0 } else { 1 }
        }
    }
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
            let _ = rt.block_on(OutEvent::event_loop(rx));
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
    event_tx: mpsc::Sender<(OutEvent, Option<Sender<OutEventRes>>)>,
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
        while let Some(&(when, ssrc, direction)) = state.expirations.iter().next() {
            if when > now {
                return Ok(Some(when));
            }
            state.expirations.remove(&(when, ssrc, direction));
            match direction {
                StreamDirection::StreamIn => {
                    let mut del = false;
                    if let Some(StreamTrace { in_on, in_timeout, .. }) = state.sessions.get_mut(&ssrc) {
                        if in_on.load(Ordering::SeqCst) {
                            in_on.store(false, Ordering::SeqCst);
                            let expires = Duration::from_millis(HALF_TIME_OUT);
                            let when = Instant::now() + expires;
                            *in_timeout = when;
                            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
                            state.expirations.insert((when, ssrc, direction));
                            if notify {
                                notify_one = notify;
                            }
                        } else {
                            del = true;
                        }
                    }
                    if del {
                        if let Some(StreamTrace { stream_id, register_ts, origin_trans, .. }) = state.sessions.remove(&ssrc) {
                            if let Some(InnerTrace { ssrc, user_map }) = state.inner.remove(&stream_id) {
                                info!("ssrc: {},stream_id: {}, 接收流超时 -> 清理会话",ssrc,&stream_id);
                                let server_name = SESSION.shared.server_conf.get_name().to_string();
                                let opt_net = origin_trans.map(|(addr, protocol)| NetSource::new(addr, protocol));
                                let rtp_info = RtpInfo::new(ssrc, opt_net, server_name);
                                let stream_info = BaseStreamInfo::new(rtp_info, stream_id, register_ts);
                                let stream_state = StreamState::new(stream_info, user_map.len() as u32);
                                let _ = SESSION.shared.event_tx.clone().send((OutEvent::StreamInTimeout(stream_state), None)).await.hand_log(|msg| error!("{msg}"));
                            }
                        }
                    }
                }
                StreamDirection::StreamOut => {
                    if let Some(StreamTrace { stream_id, .. }) = state.sessions.get(&ssrc) {
                        if let Some(InnerTrace { user_map, .. }) = state.inner.get(stream_id) {
                            if user_map.len() == 0 {
                                state.inner.remove(stream_id);
                                if let Some(stream_trace) = state.sessions.remove(&ssrc) {
                                    info!("ssrc: {},stream_id: {}, 流空闲超时 -> 清理会话",ssrc,&stream_trace.stream_id);
                                    let opt_net = stream_trace.origin_trans.map(|(addr, protocol)| NetSource::new(addr, protocol));
                                    let rtp_info = RtpInfo::new(ssrc, opt_net, SESSION.shared.server_conf.get_name().clone());
                                    let stream_info = BaseStreamInfo::new(rtp_info, stream_trace.stream_id, stream_trace.register_ts);
                                    let _ = SESSION.shared.event_tx.clone().try_send((OutEvent::StreamOutIdle(stream_info), None)).hand_log(|msg| error!("{msg}"));
                                }
                            }
                        }
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

#[allow(dead_code)]
struct StreamTrace {
    stream_id: String,
    //是否流输入：时间轮扫描-false-流断开-移除...
    in_on: AtomicBool,
    in_timeout: Instant,
    in_expires: Duration,
    //无人使用时，流关闭策略：Some-到期关闭（默认），Duration zero 立即关闭；None-不关闭；
    out_expires: Option<Duration>,
    stream_ch: Channel,
    register_ts: u32,
    //addr , protocol
    origin_trans: Option<(String, String)>,
    media_map: HashMap<u8, Media>,
    rtp_payload_type: u8,
    event_channel: (broadcast::Sender<InEvent>, Arc<Mutex<broadcast::Receiver<InEvent>>>),
    // 缓存视音频编码类型？
    //video
    //audio
    //转换流协议
    flv:bool,
    hls:Option<HlsPiece>,
}

struct InnerTrace {
    ssrc: u32,
    user_map: HashMap<SocketAddr, UserTrace>,
}

#[allow(dead_code)]
struct UserTrace {
    token: String,
    request_time: u32,
    play_type: PlayType,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
enum StreamDirection {
    //监听流注册/输入
    StreamIn,
    //监听流输出【有无观看】
    StreamOut,
}

///自定义会话信息
struct State {
    //ssrc:StreamTrace
    sessions: HashMap<u32, StreamTrace>,
    //stream_id:InnerTrace
    inner: HashMap<String, InnerTrace>,
    //(ts,ssrc,StreamDirection)
    expirations: BTreeSet<(Instant, u32, StreamDirection)>,
}

impl State {
    //获取下一个过期瞬间刻度
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations.first().map(|expiration| expiration.0)
    }
}

type CrossbeamChannel = (crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>);
type BroadcastChannel = (broadcast::Sender<FrameData>, broadcast::Receiver<FrameData>);

pub struct HalfChannel {
    pub rtp_rx: crossbeam_channel::Receiver<Packet>,
    pub flv_tx: broadcast::Sender<FrameData>,
    pub hls_tx: broadcast::Sender<FrameData>,
}

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

#[cfg(test)]
#[allow(dead_code)]
mod test {
    use std::collections::HashMap;

    #[test]
    fn test() {
        #[derive(Debug)]
        struct Inner {
            id: u32,
            map: HashMap<u8, u8>,
        }
        let mut map = HashMap::new();
        map.insert(1, 2);
        let mut inner = Inner { id: 38, map };
        let inner1 = &mut inner;

        let mut map2 = HashMap::new();
        map2.insert(3, 4);
        inner1.map = map2;

        println!("{:?}", inner);
    }
}