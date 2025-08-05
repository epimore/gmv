/// 1. 插入需监听的media信息【启动监听媒体流输入是否超时】，发送ssrc给rtp接收端监听;
/// 2. rtp接收端监听ssrc同时订阅事件获取StreamConfig；
/// 3. 插入media ext信息【此时session服务应发送指令给设备推流】;
/// 4. 等待设备推送rtp媒体流注册;
/// 4.1 设备推送rtp媒体流注册，发布事件推送StreamConfig；
/// 4.1.1 根据media信息启动媒体流处理，
/// 4.1.2 回调session服务，推送流注册事件,
/// 4.1.3 监听媒体流输出是否闲置超时【根据配置选择是否关闭监听媒体流处理】;
/// 4.1.3.1 闲置超时，回调session服务，推送接收流超时事件，关闭监听媒体流处理;
/// 4.1.3.2 点播媒体流,回调session服务,推送点播事件鉴权，通过输出媒体流，否则返回401;
/// 4.2 设备推送rtp媒体流[注册]超时，回调session服务，推送接收流超时事件，关闭监听媒体流处理;

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};


use crate::general::cfg;
use crate::general::cfg::ServerConf;
use crate::io::hook_handler::{OutEvent, OutEventRes};
use crate::media::context::event::muxer::{CloseMuxer, MuxerEvent};
use crate::media::context::event::ContextEvent;
use crate::media::context::format::flv::FlvPacket;
use crate::media::rtp::RtpPacket;
use crate::state::layer::converter_layer::ConverterLayer;
use crate::state::layer::output_layer::OutputLayer;
use crate::state::msg::StreamConfig;
use crate::state::{HALF_TIME_OUT, RTP_BUFFER_SIZE, STREAM_IDLE_TIME_OUT};
use common::bus;
use common::chrono::{Local, Timelike};
use common::exception::{GlobalError, GlobalResult, GlobalResultExt};
use common::log::{error, info, warn};
use common::net::state::Association;
use common::once_cell::sync::Lazy;
use common::tokio;
use common::tokio::sync::oneshot::Sender;
use common::tokio::sync::{broadcast, mpsc, Notify};
use common::tokio::time;
use common::tokio::time::Instant;
use parking_lot::RwLock;
use shared::info::format::MuxerType;
use shared::info::io::PlayType;
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaExt;
use shared::info::obj::{BaseStreamInfo, NetSource, RtpInfo, StreamState};

static SESSION: Lazy<Session> = Lazy::new(|| Session::init());


pub fn insert_media(stream_config: MediaStreamConfig) -> GlobalResult<u32> {
    let mut state = SESSION.shared.state.write();
    let ssrc = stream_config.ssrc;
    if !state.sessions.contains_key(&ssrc) {
        let expires = Duration::from_millis(HALF_TIME_OUT);
        let when = Instant::now() + expires;
        let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
        state.expirations.insert((when, ssrc, StreamDirection::StreamIn));
        let stream_id = stream_config.stream_id;
        let stream_conf = cfg::StreamConf::init_by_conf();
        let out_expires: &i32 = stream_conf.get_expires();
        let out_expires = match stream_config.expires {
            None => {
                build_out_expires(*out_expires)
            }
            Some(val) => {
                build_out_expires(val)
            }
        };
        let export = OutputLayer::bean_to_layer(stream_config.export)?;
        let stream_trace = StreamTrace {
            stream_id: stream_id.clone(),
            in_on: AtomicBool::new(true),
            in_timeout: when,
            in_expires: expires,
            out_expires,
            rtp_channel: crossbeam_channel::bounded(RTP_BUFFER_SIZE * 10),
            register_ts: 0,
            origin_trans: None,
            mpsc_bus: bus::mpsc::TypedMessageBus::new(),
            broadcast_bus: bus::broadcast::TypedMessageBus::new(),
            converter: ConverterLayer::bean_to_layer(stream_config.converter),
            media_ext: None,
            export,
        };
        state.sessions.insert(ssrc, stream_trace);
        let inner = InnerTrace { ssrc, user_map: Default::default() };
        state.inner.insert(stream_id, inner);
        drop(state);
        if notify {
            SESSION.shared.background_task.notify_one();
        }
        Ok(ssrc)
    } else { Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC已存在", ssrc), |msg| error!("{msg}"))) }
}

pub fn insert_media_ext(ssrc: u32, media_ext: MediaExt) -> GlobalResult<()> {
    let mut state = SESSION.shared.state.write();
    match state.sessions.entry(ssrc) {
        Entry::Occupied(mut occ) => {
            let stream_trace = occ.get_mut();
            stream_trace.media_ext = Some(media_ext);
            Ok(())
        }
        Entry::Vacant(_) => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
    }
}

pub fn sub_bus_broadcast_channel<T>(ssrc: &u32) -> GlobalResult<bus::broadcast::TypedReceiver<T>>
where
    T: Send + Sync + 'static + Clone,
{
    let state = SESSION.shared.state.read();
    match state.sessions.get(ssrc) {
        None => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
        Some(st) => {
            let receiver = st.broadcast_bus.sub_type_channel::<T>();
            Ok(receiver)
        }
    }
}
pub fn try_publish_mpsc<T>(ssrc: &u32, t: T) -> GlobalResult<()>
where
    T: Send + Sync + 'static,
{
    let state = SESSION.shared.state.read();
    match state.sessions.get(ssrc) {
        None => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
        Some(st) => {
            st.mpsc_bus.try_publish(t).hand_log(|msg| error!("{msg}"))
        }
    }
}
pub fn sub_bus_mpsc_channel<T>(ssrc: &u32) -> GlobalResult<bus::mpsc::TypedReceiver<T>>
where
    T: Send + Sync + 'static,
{
    let state = SESSION.shared.state.read();
    match state.sessions.get(ssrc) {
        None => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
        Some(st) => {
            let receiver = st.mpsc_bus.sub_type_channel::<T>().hand_log(|msg| error!("{msg}"))?;
            Ok(receiver)
        }
    }
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

//返回rtp_tx
pub fn refresh(ssrc: u32, bill: &Association, payload_type: u8) -> Option<(crossbeam_channel::Sender<RtpPacket>, crossbeam_channel::Receiver<RtpPacket>)> {
    let guard = SESSION.shared.state.read();
    if let Some(stream_trace) = guard.sessions.get(&ssrc) {
        if !stream_trace.in_on.load(Ordering::SeqCst) {
            stream_trace.in_on.store(true, Ordering::SeqCst);
        }
        //流注册
        if stream_trace.origin_trans.is_none() {
            match stream_trace.media_ext.as_ref() {
                None => { error!("ssrc = {},尚未协商rtp sdp信息", ssrc); }
                Some(media_ext) => {
                    match media_ext.tp_code == payload_type {
                        true => {
                            if let Ok(converter_event_rx) = stream_trace.mpsc_bus.sub_type_channel::<ContextEvent>().hand_log(|msg| error!("{msg}")) {
                                let stream_config = StreamConfig {
                                    converter: stream_trace.converter.clone(),
                                    media_ext: stream_trace.media_ext.clone().unwrap(),
                                    rtp_rx: stream_trace.rtp_channel.1.clone(),
                                    context_event_rx: converter_event_rx,
                                };
                                let _ = stream_trace.mpsc_bus.try_publish(stream_config).hand_log(|msg| error!("{msg}"));
                            }
                            drop(guard);
                            return stream_register(ssrc, bill);
                        }
                        false => {
                            warn!("ssrc = {},payload_type不匹配:sdp_payload_type = {},stream_payload_type = {}", ssrc,media_ext.tp_code,payload_type);
                        }
                    }
                }
            }
        } else {
            return Some((stream_trace.rtp_channel.0.clone(), stream_trace.rtp_channel.1.clone()));
        };
    }
    None
}

fn stream_register(ssrc: u32, bill: &Association) -> Option<(crossbeam_channel::Sender<RtpPacket>, crossbeam_channel::Receiver<RtpPacket>)> {
    let mut guard = SESSION.shared.state.write();
    let state = &mut *guard;
    let next_expiration = state.next_expiration();
    if let Some(stream_trace) = state.sessions.get_mut(&ssrc) {
        //首次流闲置超时，非永不超时则-默认6秒
        if let Some(mut expires) = stream_trace.out_expires {
            if expires == Duration::default() { expires = Duration::from_millis(STREAM_IDLE_TIME_OUT); }
            let when = Instant::now() + expires;
            let notify = next_expiration.map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, ssrc, StreamDirection::StreamOut(MuxerType::None)));
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

        let _ = SESSION.shared.event_tx.clone().try_send((OutEvent::StreamRegister(stream_info), None)).hand_log(|msg| error!("{msg}"));
        stream_trace.register_ts = time;

        return Some((stream_trace.rtp_channel.0.clone(), stream_trace.rtp_channel.1.clone()));
    }
    None
}

pub fn get_flv_rx(ssrc: &u32) -> GlobalResult<broadcast::Receiver<Arc<FlvPacket>>> {
    let guard = SESSION.shared.state.read();
    match guard.sessions.get(ssrc) {
        None => {
            Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},SSRC不存在或已超时丢弃", ssrc), |msg| error!("{msg}")))
        }
        Some(stream_trace) => {
            match &stream_trace.converter.muxer.flv {
                None => {
                    Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},对应的flv muxer未开启", ssrc), |msg| error!("{msg}")))
                }
                Some(flv_layer) => {
                    Ok(flv_layer.tx.subscribe())
                }
            }
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
//所有输出皆通过此计算是否idle：如无用户，如gb28181转发，则stream_id/user_token = ssrc
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
                if let Some(ut) = user_map.remove(&remote_addr) {
                    if user_map.len() == 0 {
                        if let Some(StreamTrace { out_expires, .. }) = state.sessions.get(ssrc) {
                            if let Some(timeout) = out_expires {
                                let when = Instant::now() + *timeout;
                                state.expirations.insert((when, *ssrc, StreamDirection::StreamOut(ut.play_type.get_type())));
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
                StreamDirection::StreamOut(mux_tp) => {
                    if let Some(StreamTrace { stream_id, converter, mpsc_bus, .. }) = state.sessions.get_mut(&ssrc) {
                        converter.muxer.close_by_muxer_type(&mux_tp);
                        if let Some(cm) = CloseMuxer::from_muxer_type(&mux_tp) {
                            let _ = mpsc_bus.try_publish(MuxerEvent::Close(cm)).hand_log(|msg| error!("{msg}"));
                        }
                        if !converter.muxer.check_empty() {
                            continue;
                        }
                        state.inner.remove(stream_id);
                        if let Some(stream_trace) = state.sessions.remove(&ssrc) {
                            info!("ssrc: {},stream_id: {}, 流空闲超时 -> 清理会话",ssrc,&stream_trace.stream_id);
                            let opt_net = stream_trace.origin_trans.map(|(addr, protocol)| NetSource::new(addr, protocol));
                            let rtp_info = RtpInfo::new(ssrc, opt_net, SESSION.shared.server_conf.get_name().clone());
                            let stream_info = BaseStreamInfo::new(rtp_info, stream_trace.stream_id, stream_trace.register_ts);
                            let _ = SESSION.shared.event_tx.clone().try_send((OutEvent::StreamIdle(stream_info), None)).hand_log(|msg| error!("{msg}"));
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
    rtp_channel: CrossbeamChannel,
    register_ts: u32,
    //addr , udp/tcp protocol
    origin_trans: Option<(String, String)>,
    mpsc_bus: bus::mpsc::TypedMessageBus,
    broadcast_bus: bus::broadcast::TypedMessageBus, //回调移除未使用的协议????
    converter: ConverterLayer,
    media_ext: Option<MediaExt>,
    export: OutputLayer,
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
    StreamOut(MuxerType),
}
// #[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
// enum MuxerType {
//     None,
//     Flv,
//     Mp4,
//     Ts,
//     Frame,
//     RtpPs,
//     RtpEnc,
//     RtpFrame,
// }
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

type CrossbeamChannel = (crossbeam_channel::Sender<RtpPacket>, crossbeam_channel::Receiver<RtpPacket>);
