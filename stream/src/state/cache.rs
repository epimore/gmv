use std::collections::{BTreeSet, HashMap};
use std::collections::hash_map::Entry;
use std::net::{SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use rtp::packet::Packet;

use common::chrono::{Local, Timelike};
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::error;
use common::net::state::Association;
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::{broadcast, mpsc, Notify};
use common::tokio::sync::oneshot::Sender;
use common::tokio::time;
use common::tokio::time::Instant;

use crate::biz::api::SsrcLisDto;
use crate::biz::call::{BaseStreamInfo, NetSource, RtpInfo, StreamState};
use crate::coder::FrameData;
use crate::container::PlayType;
use crate::general::cfg;
use crate::general::mode::{BUFFER_SIZE, HALF_TIME_OUT, Media, ServerConf};
use crate::io::hook_handler::{Event, EventRes};

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());

pub fn insert_media_type(ssrc: u32, media_type: HashMap<u8, Media>) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    match state.sessions.entry(ssrc) {
        Entry::Occupied(mut occ) => {
            let stream_trace = occ.get_mut();
            stream_trace.media_map = media_type;
            Ok(())
        }
        Entry::Vacant(_) => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
    }
}

//return rtp_rx,media_map,flv,hls
pub fn get_rx_media_type(ssrc: &u32) -> Option<(crossbeam_channel::Receiver<Packet>, HashMap<u8, Media>, bool, bool)> {
    let state = SESSION.shared.state.read();
    state.sessions.get(ssrc).map(|mt| (mt.stream_ch.get_rtp_rx(), mt.media_map.clone(), mt.flv, mt.hls))
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

pub fn insert(ssrc_lis: SsrcLisDto, channel: Channel) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    let ssrc = ssrc_lis.ssrc;
    if !state.sessions.contains_key(&ssrc) {
        let expires = Duration::from_millis(HALF_TIME_OUT);
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc, StreamDirection::StreamIn));
        let stream_id = ssrc_lis.stream_id;
        let stream_conf = cfg::StreamConf::init_by_conf();
        let out_expires: &i32 = stream_conf.get_expires();
        let out_expires = match ssrc_lis.expires {
            None => {
                build_out_expires(*out_expires)
            }
            Some(val) => {
                build_out_expires(val)
            }
        };
        let flv = match ssrc_lis.flv {
            None => {
                *(stream_conf.get_flv())
            }
            Some(b) => {
                b
            }
        };
        let hls = match ssrc_lis.hls {
            None => {
                *(stream_conf.get_hls())
            }
            Some(b) => {
                b
            }
        };
        let stream_trace = StreamTrace {
            stream_id: stream_id.clone(),
            in_on: AtomicBool::new(true),
            in_timeout: when,
            in_expires: expires,
            out_expires,
            stream_ch: channel,
            register_ts: 0,
            origin_trans: None,
            media_map: Default::default(),
            flv,
            hls,
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
pub fn refresh(ssrc: u32, bill: &Association) -> Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)> {
    let guard = SESSION.shared.state.read();
    if let Some(stream_trace) = guard.sessions.get(&ssrc) {
        if !stream_trace.in_on.load(Ordering::SeqCst) {
            stream_trace.in_on.store(true, Ordering::SeqCst);
        }
        //流首次注册
        return if stream_trace.register_ts == 0 {
            drop(guard);
            //回调流注册时-事件
            event_stream_in(ssrc, bill)
        } else {
            Some((stream_trace.stream_ch.rtp_channel.0.clone(), stream_trace.stream_ch.rtp_channel.1.clone()))
        };
    }
    None


    // if let Some((on, _when, stream_id, _expires, channel, reported_time, _info, _media)) = guard.sessions.get(&ssrc) {
    //     if let Some((_ssrc, _flv_sets, _hls_sets, _record)) = guard.inner.get(stream_id) {
    //         //更新流状态：时间轮会扫描流，将其置为false，若使用中则on更改为true;
    //         //增加判断流是否使用,若使用则更新流状态;目的：流空闲则断流。
    //         // if flv_sets.len() > 0 || hls_sets.len() > 0 || record.is_some() {
    //         if !on.load(Ordering::SeqCst) {
    //             on.store(true, Ordering::SeqCst);
    //         }
    //         // }
    //     }
    //     return if reported_time == &0 {
    //         drop(guard);
    //         //回调流注册时-事件
    //         event_stream_in(ssrc, bill)
    //     } else {
    //         Some((channel.rtp_channel.0.clone(), channel.rtp_channel.1.clone()))
    //     };
    // }
    // None
}

pub fn get_stream_id(ssrc: &u32) -> Option<String> {
    let guard = SESSION.shared.state.read();
    guard.sessions.get(ssrc).map(|val| val.stream_id.clone())
}

fn event_stream_in(ssrc: u32, bill: &Association) -> Option<(crossbeam_channel::Sender<Packet>, crossbeam_channel::Receiver<Packet>)> {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    let next_expiration = state.next_expiration();
    if let Some(stream_trace) = state.sessions.get_mut(&ssrc) {
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
        let _ = SESSION.shared.event_tx.clone().try_send((Event::StreamIn(stream_info), None)).hand_log(|msg| error!("{msg}"));
        stream_trace.register_ts = time;
        return Some((stream_trace.stream_ch.get_rtp_tx(), stream_trace.stream_ch.get_rtp_rx()));
    }
    None

    // if let Some((_on, _when, stream_id, _expires, channel, reported_time, info, _)) = guard.sessions.get_mut(&ssrc) {
    //     let remote_addr_str = bill.get_remote_addr().to_string();
    //     let protocol_addr = bill.get_protocol().get_value().to_string();
    //     *info = Some((remote_addr_str.clone(), protocol_addr.clone()));
    //     let rtp_info = RtpInfo::new(ssrc, Some(protocol_addr), Some(remote_addr_str), SESSION.shared.server_conf.get_name().clone());
    //     let time = SystemTime::now().duration_since(UNIX_EPOCH).expect("Time went backwards").as_secs() as u32;
    //     let stream_info = BaseStreamInfo::new(rtp_info, stream_id.clone(), time);
    //     let _ = SESSION.shared.event_tx.clone().try_send((Event::StreamIn(stream_info), None)).hand_log(|msg| error!("{msg}"));
    //     *reported_time = time;
    //     return Some((channel.rtp_channel.0.clone(), channel.rtp_channel.1.clone()));
    // }
    // return None;
}

//外层option判断ssrc是否存在，里层判断是否需要rtp/hls协议
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

pub fn get_event_tx() -> mpsc::Sender<(Event, Option<Sender<EventRes>>)> {
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

    // if let Some((_, flv_sets, hls_sets, _)) = state.inner.get_mut(stream_id) {
    //     match &play_type[..] {
    //         "flv" => { if in_out { flv_sets.insert(user_token); } else { flv_sets.remove(&user_token); } }
    //         "hls" => { if in_out { hls_sets.insert(user_token); } else { hls_sets.remove(&user_token); } }
    //         _ => {}
    //     }
    // }
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


    // match guard.inner.get(stream_id) {
    //     None => {
    //         None
    //     }
    //     Some((ssrc, flv_tokens, hls_tokens, _)) => {
    //         match guard.sessions.get(ssrc) {
    //             Some((_, _ts, stream_id, _dur, _ch, stream_in_reported_time, Some((origin_addr, protocol)), _)) => {
    //                 let server_name = SESSION.shared.server_conf.get_name().to_string();
    //                 let rtp_info = RtpInfo::new(*ssrc, Some(protocol.to_string()), Some(origin_addr.to_string()), server_name);
    //                 let stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *stream_in_reported_time);
    //                 Some((stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32))
    //             }
    //             _ => { None }
    //         }
    //     }
    // }
}

// pub fn remove_user(stream_id: &String, remote_addr: &SocketAddr) {
//     let mut guard = SESSION.shared.state.write();
//     let state = &mut *guard;
//     if let Some(InnerTrace { user_map, .. }) = state.inner.get_mut(stream_id) {
//         user_map.remove(remote_addr);
//     }

// if let Some(InnerTrace { ssrc, .. }) = state.inner.remove(stream_id) {
//     if let Some((_, when, _, _, _, _, _, _)) = state.sessions.remove(&ssrc) {
//         state.expirations.remove(&(when, ssrc));
//     }
// }
// }

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

    // let mut vec = Vec::new();
    // let server_name = SESSION.shared.server_conf.get_name().to_string();
    // match opt_stream_id {
    //     None => {
    //         //ssrc,(on,ts,stream_id,dur,ch,stream_in_reported_time,(origin_addr,protocol))
    //         for (ssrc, (_, _, stream_id, _, _, report_timestamp, opt_addr_protocol, _)) in &guard.sessions {
    //             let mut origin_addr = None;
    //             let mut protocol = None;
    //             if let Some((addr, proto)) = opt_addr_protocol {
    //                 origin_addr = Some(addr.clone());
    //                 protocol = Some(proto.clone());
    //             }
    //             let rtp_info = RtpInfo::new(*ssrc, protocol, origin_addr, server_name.clone());
    //             let base_stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *report_timestamp);
    //             //stream_id:(ssrc,flv-tokens,hls-tokens,record_name)
    //             if let Some((_ssrc, flv_tokens, hls_tokens, record_name)) = guard.inner.get(stream_id) {
    //                 let state = StreamState::new(base_stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32, record_name.clone());
    //                 vec.push(state);
    //             }
    //         }
    //     }
    //     Some(stream_id) => {
    //         if let Some((ssrc, flv_tokens, hls_tokens, record_name)) = &guard.inner.get(&stream_id) {
    //             if let Some((_, _, stream_id, _, _, report_timestamp, opt_addr_protocol, _)) = &guard.sessions.get(ssrc) {
    //                 let mut origin_addr = None;
    //                 let mut protocol = None;
    //                 if let Some((addr, proto)) = opt_addr_protocol {
    //                     origin_addr = Some(addr.clone());
    //                     protocol = Some(proto.clone());
    //                 }
    //                 let rtp_info = RtpInfo::new(*ssrc, protocol, origin_addr, server_name.clone());
    //                 let base_stream_info = BaseStreamInfo::new(rtp_info, stream_id.to_string(), *report_timestamp);
    //                 let state = StreamState::new(base_stream_info, flv_tokens.len() as u32, hls_tokens.len() as u32, record_name.clone());
    //                 vec.push(state);
    //             }
    //         }
    //     }
    // }
    // vec
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
                            // if let Some((_, _, stream_id, _, _, stream_in_reported_time, op, _)) = state.sessions.remove(&ssrc) {
                            if let Some(InnerTrace { ssrc, user_map }) = state.inner.remove(&stream_id) {
                                // if let Some((ssrc, flv_tokens, hls_tokens, record_name)) = state.inner.remove(&stream_id) {
                                //callback stream timeout
                                let server_name = SESSION.shared.server_conf.get_name().to_string();
                                // let mut origin_addr = None;
                                // let mut protocol = None;
                                // if let Some((origin_addr_s, protocol_s)) = origin_trans {
                                //     origin_addr = Some(origin_addr_s);
                                //     protocol = Some(protocol_s);
                                // }
                                let opt_net = origin_trans.map(|(addr, protocol)| NetSource::new(addr, protocol));
                                let rtp_info = RtpInfo::new(ssrc, opt_net, server_name);
                                let stream_info = BaseStreamInfo::new(rtp_info, stream_id, register_ts);
                                let stream_state = StreamState::new(stream_info, user_map.len() as u32);
                                let _ = SESSION.shared.event_tx.clone().send((Event::StreamInTimeout(stream_state), None)).await.hand_log(|msg| error!("{msg}"));
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
                                    let opt_net = stream_trace.origin_trans.map(|(addr, protocol)| NetSource::new(addr, protocol));
                                    let rtp_info = RtpInfo::new(ssrc, opt_net, SESSION.shared.server_conf.get_name().clone());
                                    let stream_info = BaseStreamInfo::new(rtp_info, stream_trace.stream_id, stream_trace.register_ts);
                                    let _ = SESSION.shared.event_tx.clone().try_send((Event::StreamOutIdle(stream_info), None)).hand_log(|msg| error!("{msg}"));
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
    flv: bool,
    hls: bool,
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
    //ssrc,(on,ts,stream_id,dur,ch,stream_in_reported_time,(origin_addr,protocol),(media_type,media_type_enum))
    // sessions: HashMap<u32, (AtomicBool, Instant, String, Duration, Channel, u32, Option<(String, String)>, HashMap<u8, Media>)>,
    sessions: HashMap<u32, StreamTrace>,
    //stream_id:(ssrc,flv-tokens,hls-tokens,record_name)
    // inner: HashMap<String, (u32, HashSet<String>, HashSet<String>, Option<String>)>,
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