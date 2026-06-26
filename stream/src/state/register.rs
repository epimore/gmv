use crate::general::cfg;
use crate::general::cfg::{ServerConf, StreamConf};
use crate::general::util::Placeholder;
use crate::io::local::mp4::{LocalStoreMp4Context, Mp4OutputInnerEvent};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::muxer::MuxerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::media::rtp::RtpPacket;
use crate::state::event::{ActiveEvent, Event, EventRes, InnerEvent, OutEvent};
use crate::state::layer::converter_layer::ConverterLayer;
use crate::state::layer::output_layer::OutputLayer;
use crate::state::msg::StreamConfig;
use crate::state::{RTP_BUFFER_SIZE, event};
use base::bus;
use base::cache::c100k;
use base::cache::c100k::CacheEvent;
use base::dashmap::DashMap;
use base::dashmap::mapref::entry::Entry;
use base::dashmap::mapref::one::Ref;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::net::state::Protocol;
use base::once_cell::sync::OnceCell;
use base::tokio::select;
use base::tokio::sync::oneshot::Sender;
use base::tokio::sync::{broadcast, mpsc};
use base::utils::rt::GlobalRuntime;
use log::{error, info};
use shared::enums::OptAction;
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaExt;
use shared::info::obj::{
    BaseStreamInfo, InTimeoutEventRes, NetSource, OutputEventRes, OutputStreamInfo,
    RegisterStreamInfo, RtpInfo, StreamKey, StreamPlayInfo, StreamState, UnknownStreamEvent,
};
use shared::info::output::{OutputEnum, OutputKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static REGISTER: OnceCell<Register> = OnceCell::new();
pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
const RTP_INPUT_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const UNKNOWN_STREAM_CONFIRM_MS: u64 = 2_000;
const UNKNOWN_STREAM_MIN_PACKETS: u64 = 3;
const UNKNOWN_STREAM_COOLDOWN_MS: u64 = 30_000;
const UNKNOWN_STREAM_EXPIRE_MS: u64 = 30_000;
//时间偏移：用于如mpd、playlist一次加载多个媒体片段，导致提前超时
pub const DEFAULT_OFFSET_SECOND: u8 = 4;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[inline]
fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}
type CrossbeamChannel = (
    crossbeam_channel::Sender<RtpPacket>,
    crossbeam_channel::Receiver<RtpPacket>,
);
pub struct RtpChannel {
    pub rtp_tx: crossbeam_channel::Sender<RtpPacket>,
    pub rtp_rx: crossbeam_channel::Receiver<RtpPacket>,
    pub in_has_timeout: AtomicU8, //输入流已超时n秒
    pub wait_sign_in: AtomicBool,
    pub stream_id: Arc<str>,
    pub miss_pkt: AtomicUsize,
}
impl RtpChannel {
    fn new(stream_id: Arc<str>) -> RtpChannel {
        let (rtp_tx, rtp_rx) = crossbeam_channel::bounded(RTP_BUFFER_SIZE * 10);
        Self {
            rtp_tx,
            rtp_rx,
            in_has_timeout: AtomicU8::new(0),
            wait_sign_in: AtomicBool::new(true),
            stream_id,
            miss_pkt: AtomicUsize::new(0),
        }
    }
    fn get_rtp_rx(&self) -> crossbeam_channel::Receiver<RtpPacket> {
        self.rtp_rx.clone()
    }
    fn refresh(
        &self,
        ssrc: u32,
        rtp_type: u8,
        origin_trans: (SocketAddr, Protocol),
    ) -> GlobalResult<crossbeam_channel::Sender<RtpPacket>> {
        if self.wait_sign_in.load(Ordering::Relaxed) {
            Register::get()
                .inner
                .event_tx
                .try_send((
                    Event::Inner(InnerEvent::StreamRegister(
                        rtp_type,
                        self.stream_id.clone(),
                        origin_trans,
                    )),
                    None,
                ))
                .hand_log(|msg| {
                    error!("System busy;InnerEvent: {ssrc} Stream registration send failed: {msg}")
                })?;
            self.wait_sign_in.store(false, Ordering::Relaxed);
        }
        self.in_has_timeout.store(0, Ordering::Relaxed);

        if self.rtp_tx.is_full() {
            let count = self.miss_pkt.fetch_add(1, Ordering::Relaxed);
            //延迟等待信令处理完成
            let call_io_busy = if count < 60 {
                count % 60 == 0
            } else {
                count % 300 == 0
            };
            if call_io_busy {
                Err(GlobalError::new_biz_error(
                    BaseErrorCode::IoBusy.code(),
                    "rtp channel is full,miss pkt count: ",
                    |msg| error!("ssrc: {ssrc},{msg}{count}"),
                ))?;
            }
        } else {
            self.miss_pkt.store(0, Ordering::Relaxed);
        }
        Ok(self.rtp_tx.clone())
    }
}

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    RtpGateway(u32),
    OutSession(u64),
    UnknownStream(Arc<str>),
}
#[derive(Clone, Hash, Eq, PartialEq)]
pub struct OutSession {
    pub addr: SocketAddr,
    pub token: Arc<str>,
    pub output_enum: OutputEnum,
    pub stream_id: Arc<str>,
}
#[derive(Clone)]
pub struct UnknownStreamObservation {
    pub ssrc: u32,
    pub remote_addr: SocketAddr,
    pub protocol: String,
    pub first_seen_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub packet_count: u64,
    pub last_notify_at_ms: u64,
}

impl UnknownStreamObservation {
    fn observe(&mut self, now_ms: u64) -> bool {
        self.last_seen_at_ms = now_ms;
        self.packet_count = self.packet_count.saturating_add(1);
        let confirmed = self.packet_count >= UNKNOWN_STREAM_MIN_PACKETS
            && now_ms.saturating_sub(self.first_seen_at_ms) >= UNKNOWN_STREAM_CONFIRM_MS;
        let cooled_down = self.last_notify_at_ms == 0
            || now_ms.saturating_sub(self.last_notify_at_ms) >= UNKNOWN_STREAM_COOLDOWN_MS;
        if confirmed && cooled_down {
            self.last_notify_at_ms = now_ms;
            true
        } else {
            false
        }
    }

    fn idle_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_seen_at_ms)
    }
}
pub struct StreamMetadata {
    pub ssrc: u32,
    pub output_count: OutputCount,
    pub in_wait_timeout: u8,
    pub out_idle_timeout: u8,
    // 关闭muxer或流是否已回调:不以timeout立即关闭流，可能需要流保活，如用户暂停观看；
    // 0-未回调，1-回调请求中，
    // 2-保活(继续下一轮timeout事件)，3-关闭，
    // 4-首次请求异常或超时，5-隔2s第二次请求异常或超时，6-隔4s第三次请求异常或超时；若依旧异常或超时则关闭
    pub close_has_call: AtomicU8,
    pub origin_trans: Option<(SocketAddr, Protocol)>,
    pub register_ts: u64,
    pub mpsc_bus: bus::mpsc::TypedMessageBus,
    pub broadcast_bus: bus::broadcast::TypedMessageBus,
    pub converter: ConverterLayer,
    pub media_ext: Option<MediaExt>,
    pub output: OutputLayer,
}
impl StreamMetadata {
    fn new(
        ssrc: u32,
        in_wait_timeout: u8,
        out_idle_timeout: u8,
        converter: ConverterLayer,
        output: OutputLayer,
    ) -> Self {
        StreamMetadata {
            ssrc,
            output_count: Default::default(),
            in_wait_timeout,
            out_idle_timeout,
            close_has_call: AtomicU8::new(0),
            register_ts: 0,
            origin_trans: None,
            mpsc_bus: bus::mpsc::TypedMessageBus::new(),
            broadcast_bus: bus::broadcast::TypedMessageBus::new(),
            converter,
            media_ext: None,
            output,
        }
    }
    //云端录制在init_media时，初始化输出端
    fn build_from_output_kind(
        &self,
        output_kind: OutputKind,
        ssrc: u32,
        stream_id: Arc<str>,
        event_tx: mpsc::Sender<(Event, Option<Sender<EventRes>>)>,
    ) -> Option<ActiveEvent> {
        match output_kind {
            OutputKind::HttpFlv(_) => None,
            OutputKind::Rtmp(_) => {
                unimplemented!()
            }
            OutputKind::DashFmp4(_) => None,
            OutputKind::HlsFmp4(_) => None,
            OutputKind::HlsTs(_) => None,
            OutputKind::Rtsp(_) => {
                unimplemented!()
            }
            OutputKind::Gb28181Frame(_) => {
                unimplemented!()
            }
            OutputKind::Gb28181Ps(_) => {
                unimplemented!()
            }
            OutputKind::WebRtc(_) => {
                unimplemented!()
            }
            OutputKind::LocalMp4(info) => {
                let context = LocalStoreMp4Context {
                    path: info.path,
                    ssrc,
                    file_name: stream_id.clone(),
                    pkt_rx: self.converter.muxer.get_rx(MuxerEnum::Mp4).unwrap(),
                    record_event_tx: event_tx,
                    inner_event_rx: self
                        .mpsc_bus
                        .sub_type_channel::<Mp4OutputInnerEvent>()
                        .unwrap(),
                    file_size: 0,
                    ts: 0,
                    state: 0,
                };
                Some(ActiveEvent::LocalStoreMp4(context))
            }
            OutputKind::LocalTs(_) => {
                unimplemented!()
            }
            OutputKind::DashMp4(_) => None,
        }
    }
}
pub struct Register {
    pub inner: Arc<Inner>,
}
pub struct Inner {
    pub time_schedule: c100k::Cache<TimeScheduleKey>,
    //key:ssrc
    pub rtp_gateway_map: DashMap<u32, RtpChannel>,
    //key:stream_id
    pub stream_metadata_map: DashMap<Arc<str>, StreamMetadata>,
    pub out_session_map: DashMap<u64, OutSession>,
    pub unknown_stream_map: DashMap<Arc<str>, UnknownStreamObservation>,
    //key:(token,stream_id),value: key-OutputEnum,value-playCount
    pub user_token_map: DashMap<(Arc<str>, Arc<str>), DashMap<OutputEnum, u32>>,
    pub server_conf: ServerConf,
    pub stream_conf: StreamConf,
    pub event_tx: mpsc::Sender<(Event, Option<Sender<EventRes>>)>,
}
impl Register {
    fn get() -> &'static Register {
        REGISTER.get().expect("Register not initialized")
    }

    pub fn check_token(user_token_stream_id: &(Arc<str>, Arc<str>)) -> bool {
        Self::get()
            .inner
            .user_token_map
            .contains_key(user_token_stream_id)
    }

    //插入/移除muxer使用量
    pub fn handle_stream_metadata_map_output(
        act: OptAction,
        stream_id: &Arc<str>,
        output_enum: OutputEnum,
    ) {
        let arc = Self::get().inner.clone();
        arc.stream_metadata_map
            .get(stream_id)
            .map(|meta| match act {
                OptAction::Insert => meta.output_count.add(output_enum),
                OptAction::Remove => meta.output_count.subtract(output_enum),
            });
    }
    pub fn observe_unknown_rtp(ssrc: u32, remote_addr: SocketAddr, protocol: Protocol) {
        let arc = Self::get().inner.clone();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        let protocol = protocol.get_value().to_string();
        let key: Arc<str> = Arc::from(format!("{ssrc}|{remote_addr}|{protocol}"));
        let mut schedule_expiry = false;
        let event = match arc.unknown_stream_map.entry(key.clone()) {
            Entry::Vacant(vacant) => {
                schedule_expiry = true;
                vacant.insert(UnknownStreamObservation {
                    ssrc,
                    remote_addr,
                    protocol,
                    first_seen_at_ms: now,
                    last_seen_at_ms: now,
                    packet_count: 1,
                    last_notify_at_ms: 0,
                });
                None
            }
            Entry::Occupied(mut occupied) => {
                let observation = occupied.get_mut();
                if observation.observe(now) {
                    Some(UnknownStreamEvent {
                        media_node_id: arc.server_conf.name.clone(),
                        ssrc: observation.ssrc,
                        remote_addr: observation.remote_addr.to_string(),
                        protocol: observation.protocol.clone(),
                        first_seen_at_ms: observation.first_seen_at_ms,
                        last_seen_at_ms: observation.last_seen_at_ms,
                        packet_count: observation.packet_count,
                        reason: "unknown_ssrc_after_media_restart".to_string(),
                    })
                } else {
                    None
                }
            }
        };
        if schedule_expiry {
            let _ = arc.time_schedule.insert(
                TimeScheduleKey::UnknownStream(key),
                Duration::from_millis(UNKNOWN_STREAM_EXPIRE_MS),
            );
        }
        if let Some(event) = event {
            let _ = arc
                .event_tx
                .try_send((Event::Out(OutEvent::StreamUnknown(event)), None))
                .hand_log(|msg| error!("{msg}: ssrc={ssrc}"));
        }
    }

    pub fn expire_unknown_stream(key: Arc<str>, inner: &Inner) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        let Some(observation) = inner.unknown_stream_map.get(&key) else {
            return;
        };
        let idle_ms = observation.idle_ms(now);
        drop(observation);
        if idle_ms >= UNKNOWN_STREAM_EXPIRE_MS {
            inner.unknown_stream_map.remove(&key);
        } else {
            let _ = inner.time_schedule.insert(
                TimeScheduleKey::UnknownStream(key),
                Duration::from_millis(UNKNOWN_STREAM_EXPIRE_MS - idle_ms),
            );
        }
    }

    pub fn handle_rtp_in_timeout(ssrc: u32, inner: &Inner) {
        if let Some(rc) = inner.rtp_gateway_map.get(&ssrc) {
            if let Some(meta) = inner.stream_metadata_map.get(&rc.stream_id) {
                let timeout_count = rc
                    .in_has_timeout
                    .fetch_add(1, Ordering::Relaxed)
                    .saturating_add(1);
                if timeout_count < meta.in_wait_timeout {
                    let _ = inner
                        .time_schedule
                        .insert(TimeScheduleKey::RtpGateway(ssrc), RTP_INPUT_CHECK_INTERVAL)
                        .hand_log(|msg| error!("{msg}"));
                } else {
                    let stream_info = Self::build_base_stream_info(
                        &meta,
                        inner.server_conf.name.clone(),
                        inner.server_conf.proxy_addr.clone(),
                        rc.stream_id.clone().to_string(),
                    );
                    let state = StreamState::new(stream_info, meta.output_count.get_out_count());
                    let _ = inner
                        .event_tx
                        .try_send((Event::Out(OutEvent::StreamInTimeout(state)), None))
                        .hand_log(|msg| error!("{msg}"));
                }
            }
        }
    }
    pub fn close_stream_by_input(state: StreamState, res: InTimeoutEventRes) {
        let arc = Self::get().inner.clone();
        match res {
            InTimeoutEventRes::KeepAlive => {
                arc.time_schedule.insert(
                    TimeScheduleKey::RtpGateway(state.base_stream_info.rtp_info.ssrc),
                    DEFAULT_EXPIRES,
                );
            }
            InTimeoutEventRes::CloseAll => {
                let stream_id = Arc::from(state.base_stream_info.stream_id);
                arc.stream_metadata_map.remove(&stream_id);
                arc.rtp_gateway_map
                    .remove(&state.base_stream_info.rtp_info.ssrc);
            }
        }
    }
    pub fn close_stream_by_output(info: OutputStreamInfo, res: OutputEventRes) {
        let arc = Self::get().inner.clone();
        match res {
            OutputEventRes::KeepMuxer => {
                let expire_id = next_id();
                arc.time_schedule
                    .insert(TimeScheduleKey::OutSession(expire_id), DEFAULT_EXPIRES);
                let out_session = OutSession {
                    addr: SocketAddr::placeholder(),
                    token: Default::default(),
                    output_enum: info.play_type,
                    stream_id: Arc::from(info.base_stream_info.stream_id),
                };
                arc.out_session_map.insert(expire_id, out_session);
            }
            OutputEventRes::CloseMuxer => {
                let muxer_enum = MuxerEnum::from_output_enum(info.play_type);
                let stream_id = Arc::from(info.base_stream_info.stream_id);
                arc.stream_metadata_map.get_mut(&stream_id).map(|mut meta| {
                    let size = meta.output_count.get_muxer_size(info.play_type);
                    if size == 0 {
                        info!(
                            "ssrc = {},stream id = {} close muxer: {:?}",
                            meta.ssrc, stream_id, muxer_enum
                        );
                        meta.converter.muxer.close_by_muxer_type(muxer_enum);
                        let _ = meta
                            .mpsc_bus
                            .try_publish(MuxerEvent::Close(muxer_enum))
                            .hand_log(|msg| info!("{msg}"));
                    }
                });
            }
            OutputEventRes::CloseAll => {
                let stream_id = Arc::from(info.base_stream_info.stream_id);
                info!(
                    "ssrc = {},stream id = {} close",
                    info.base_stream_info.rtp_info.ssrc, stream_id
                );
                arc.stream_metadata_map.remove(&stream_id);
                arc.rtp_gateway_map
                    .remove(&info.base_stream_info.rtp_info.ssrc);
            }
        }
    }
    pub fn clean_play_token(expire_id: u64) {
        let arc = Self::get().inner.clone();

        match arc.out_session_map.remove(&expire_id) {
            None => {}
            Some(os) => {
                let mut user_token = None;
                //清理用户
                match arc
                    .user_token_map
                    .entry((os.1.token, os.1.stream_id.clone()))
                {
                    Entry::Occupied(occ) => {
                        let to_del = match occ.get().entry(os.1.output_enum) {
                            Entry::Occupied(mut i_occ) => {
                                if *i_occ.get() == 1 {
                                    i_occ.remove();
                                    true
                                } else {
                                    *i_occ.get_mut() -= 1;
                                    false
                                }
                            }
                            Entry::Vacant(_) => false,
                        };
                        if to_del && occ.get().len() == 0 {
                            let ((token, _), _) = occ.remove_entry();
                            // event user off play
                            if !token.is_empty() {
                                user_token = Some(token.to_string());
                            }
                        }
                    }
                    Entry::Vacant(_) => {}
                }
                //清理媒体资源
                match arc.stream_metadata_map.get(&os.1.stream_id) {
                    None => {}
                    Some(meta) => {
                        let base_stream_info = Self::build_base_stream_info(
                            &meta,
                            arc.server_conf.name.clone(),
                            arc.server_conf.proxy_addr.clone(),
                            os.1.stream_id.to_string(),
                        );
                        // event user off play
                        if let Some(token) = user_token {
                            let play_info = StreamPlayInfo::new(
                                base_stream_info.clone(),
                                None,
                                token,
                                os.1.output_enum,
                            );
                            let _ = arc
                                .event_tx
                                .try_send((Event::Out(OutEvent::OffPlay(play_info)), None))
                                .hand_log(|msg| error!("{msg}"));
                        }
                        if meta.output_count.subtract(os.1.output_enum) == 0 {
                            let output_stream_info = OutputStreamInfo::new(
                                base_stream_info,
                                os.1.output_enum,
                                meta.output_count.get_out_count(),
                            );
                            let _ = arc
                                .event_tx
                                .try_send((
                                    Event::Out(OutEvent::StreamIdle(output_stream_info)),
                                    None,
                                ))
                                .hand_log(|msg| error!("{msg}"));
                        }
                    }
                }
            }
        }
    }
    pub fn listen_output_timeout(
        stream_id: Arc<str>,
        output_enum: OutputEnum,
        user_token: Arc<str>,
        remote_addr: SocketAddr,
        time_offset_sec: u8,
    ) {
        let arc = Self::get().inner.clone();
        if let Some(meta) = arc.stream_metadata_map.get(&stream_id) {
            let expire_id = next_id();
            arc.out_session_map.insert(
                expire_id,
                OutSession {
                    addr: remote_addr,
                    token: user_token,
                    output_enum,
                    stream_id,
                },
            );
            let timeout_sec = time_offset_sec.saturating_add(meta.out_idle_timeout);
            arc.time_schedule.insert(
                TimeScheduleKey::OutSession(expire_id),
                Duration::from_secs(timeout_sec as u64),
            );
        }
    }

    pub fn insert_out_token(
        stream_id: Arc<str>,
        output_enum: OutputEnum,
        user_token: Arc<str>,
    ) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();
        match arc.stream_metadata_map.get(&stream_id) {
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SSRC不存在或已超时丢弃",
                |msg| error!("stream_id = {}; {msg}", stream_id),
            ))?,
            Some(meta) => {
                meta.output_count.add(output_enum);
            }
        }
        match arc.user_token_map.entry((user_token, stream_id)) {
            Entry::Occupied(occ) => match occ.get().entry(output_enum) {
                Entry::Occupied(mut i_occ) => {
                    *i_occ.get_mut() += 1;
                }
                Entry::Vacant(i_vac) => {
                    i_vac.insert(1);
                }
            },
            Entry::Vacant(vac) => {
                let map = DashMap::new();
                map.insert(output_enum, 1);
                vac.insert(map);
            }
        }
        Ok(())
    }
    fn build_rtp_info(meta: &StreamMetadata, server_name: String, proxy_addr: String) -> RtpInfo {
        let net_source = meta
            .origin_trans
            .map(|(addr, prot)| NetSource::new(addr.to_string(), prot.get_value().to_string()));
        RtpInfo::new(meta.ssrc, net_source, server_name, proxy_addr)
    }
    fn build_base_stream_info(
        meta: &StreamMetadata,
        server_name: String,
        proxy_addr: String,
        stream_id: String,
    ) -> BaseStreamInfo {
        let rtp_info = Self::build_rtp_info(meta, server_name, proxy_addr);
        BaseStreamInfo::new(rtp_info, stream_id, meta.register_ts)
    }

    //返回BaseStreamInfo,user_count
    pub fn get_base_stream_info_by_stream_id(stream_id: Arc<str>) -> Option<BaseStreamInfo> {
        let arc = Self::get().inner.clone();
        arc.stream_metadata_map.get(&stream_id).map(|meta| {
            let stream_info = Self::build_base_stream_info(
                &meta,
                arc.server_conf.name.clone(),
                arc.server_conf.proxy_addr.clone(),
                stream_id.to_string(),
            );
            stream_info
        })
    }
    pub fn is_exist(stream_key: StreamKey) -> bool {
        let StreamKey { stream_id, ssrc } = stream_key;
        let arc = Self::get().inner.clone();
        match stream_id {
            None => arc.rtp_gateway_map.contains_key(&ssrc),
            Some(stream_id) => {
                let stream_id: Arc<str> = Arc::from(stream_id);
                arc.stream_metadata_map
                    .get(&stream_id)
                    .is_some_and(|metadata| metadata.ssrc == ssrc)
                    && arc
                        .rtp_gateway_map
                        .get(&ssrc)
                        .is_some_and(|channel| channel.stream_id == stream_id)
            }
        }
    }

    pub fn active_stream_count() -> usize {
        Self::get().inner.stream_metadata_map.len()
    }

    pub fn get_event_tx() -> mpsc::Sender<(Event, Option<Sender<EventRes>>)> {
        Self::get().inner.event_tx.clone()
    }
    pub fn get_server_conf() -> &'static ServerConf {
        &Self::get().inner.server_conf
    }
    pub fn get_muxer_rx(
        ssrc: &u32,
        muxer_enum: MuxerEnum,
    ) -> GlobalResult<broadcast::Receiver<Arc<MuxPacket>>> {
        let arc = Self::get().inner.clone();
        match arc.rtp_gateway_map.get(&ssrc) {
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SSRC不存在或已超时丢弃",
                |msg| error!("ssrc={}; {msg}", ssrc),
            )),
            Some(rc) => match arc.stream_metadata_map.get(&rc.stream_id) {
                None => Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "SSRC不存在或已超时丢弃",
                    |msg| error!("ssrc={}; {msg}", ssrc),
                )),
                Some(meta) => meta.converter.muxer.get_rx(muxer_enum),
            },
        }
    }
    pub fn sub_bus_mpsc_channel<T>(ssrc: &u32) -> GlobalResult<bus::mpsc::TypedReceiver<T>>
    where
        T: Send + Sync + 'static,
    {
        let arc = Self::get().inner.clone();
        match arc.rtp_gateway_map.get(&ssrc) {
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SSRC不存在或已超时丢弃",
                |msg| error!("ssrc={}; {msg}", ssrc),
            )),
            Some(rc) => match arc.stream_metadata_map.get(&rc.stream_id) {
                None => Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "SSRC不存在或已超时丢弃",
                    |msg| error!("ssrc={}; {msg}", ssrc),
                )),
                Some(meta) => {
                    let receiver = meta
                        .mpsc_bus
                        .sub_type_channel::<T>()
                        .hand_log(|msg| error!("{msg}"))?;
                    Ok(receiver)
                }
            },
        }
    }
    pub fn try_publish_mpsc<T>(ssrc: u32, t: T) -> GlobalResult<()>
    where
        T: Send + Sync + 'static,
    {
        let arc = Self::get().inner.clone();
        match arc.rtp_gateway_map.get(&ssrc) {
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SSRC不存在或已超时丢弃",
                |msg| error!("ssrc={}; {msg}", ssrc),
            )),
            Some(rc) => match arc.stream_metadata_map.get(&rc.stream_id) {
                None => Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "SSRC不存在或已超时丢弃",
                    |msg| error!("ssrc={}; {msg}", ssrc),
                )),
                Some(meta) => meta.mpsc_bus.try_publish(t).hand_log(|msg| error!("{msg}")),
            },
        }
    }
    pub fn init_media_ext(ssrc: u32, media_ext: MediaExt) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();
        match arc.rtp_gateway_map.get(&ssrc) {
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SSRC不存在或已超时丢弃",
                |msg| error!("ssrc={}; {msg}", ssrc),
            )),
            Some(rc) => match arc.stream_metadata_map.entry(rc.stream_id.clone()) {
                Entry::Occupied(mut occ) => {
                    let meta = occ.get_mut();
                    meta.media_ext = Some(media_ext);
                    Ok(())
                }
                Entry::Vacant(_) => Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "SSRC不存在或已超时丢弃",
                    |msg| error!("ssrc={}; {msg}", ssrc),
                )),
            },
        }
    }
    pub fn refresh_rtp(
        ssrc: u32,
        rtp_type: u8,
        origin_trans: (SocketAddr, Protocol),
    ) -> Option<crossbeam_channel::Sender<RtpPacket>> {
        match Self::get().inner.clone().rtp_gateway_map.get(&ssrc) {
            None => None,
            Some(rc) => rc.refresh(ssrc, rtp_type, origin_trans).ok(),
        }
    }

    pub fn send_stream_config(rtp_type: u8, stream_id: Arc<str>) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();
        if let Some(meta) = arc.stream_metadata_map.get(&stream_id) {
            if let Some(media_ext) = meta.media_ext.as_ref() {
                if media_ext.type_code == rtp_type {
                    if let Some(rtp_rx) = arc
                        .rtp_gateway_map
                        .get(&meta.ssrc)
                        .map(|rtp_channel| rtp_channel.get_rtp_rx())
                    {
                        if let Ok(converter_event_rx) = meta
                            .mpsc_bus
                            .sub_type_channel::<ContextEvent>()
                            .hand_log(|msg| error!("{msg}"))
                        {
                            let stream_config = StreamConfig {
                                converter: meta.converter.clone(),
                                media_ext: meta.media_ext.clone().unwrap(),
                                rtp_rx,
                                context_event_rx: converter_event_rx,
                            };
                            let _ = meta
                                .mpsc_bus
                                .try_publish(stream_config)
                                .hand_log(|msg| error!("{msg}"));
                            let stream_info = Self::build_base_stream_info(
                                &meta,
                                arc.server_conf.name.clone(),
                                arc.server_conf.proxy_addr.clone(),
                                stream_id.to_string(),
                            );
                            let info = RegisterStreamInfo {
                                base_stream_info: stream_info,
                                code: 200,
                                msg: None,
                            };
                            let _ = arc
                                .event_tx
                                .try_send((Event::Out(OutEvent::StreamRegister(info)), None))
                                .hand_log(|msg| error!("{msg}"));
                        }
                    }
                } else {
                    let stream_info = Self::build_base_stream_info(
                        &meta,
                        arc.server_conf.name.clone(),
                        arc.server_conf.proxy_addr.clone(),
                        stream_id.to_string(),
                    );
                    let info = RegisterStreamInfo {
                        base_stream_info: stream_info,
                        code: BaseErrorCode::InvalidState.code(),
                        msg: Some(format!(
                            "Play type is not identical;sdp={},rtp={}",
                            media_ext.type_code, rtp_type
                        )),
                    };
                    let _ = arc
                        .event_tx
                        .try_send((Event::Out(OutEvent::StreamRegister(info)), None))
                        .hand_log(|msg| error!("{msg}"));
                    //释放媒体资源
                    let ssrc = meta.ssrc;
                    drop(meta);
                    arc.stream_metadata_map.remove(&stream_id);
                    arc.rtp_gateway_map.remove(&ssrc);
                }
            } else {
                error!("RTP 首包早于 SDP 扩展信息;ssrc = {}", meta.ssrc)
            }
        }
        Ok(())
    }

    pub fn build_stream_info(stream_id: Arc<str>) -> Option<BaseStreamInfo> {
        let arc = Self::get().inner.clone();
        arc.stream_metadata_map.get(&stream_id).map(|meta| {
            Self::build_base_stream_info(
                &meta,
                arc.server_conf.name.clone(),
                arc.server_conf.proxy_addr.clone(),
                stream_id.to_string(),
            )
        })
    }
    pub fn insert_origin_trans(stream_id: Arc<str>, origin_trans: (SocketAddr, Protocol)) -> bool {
        let arc = Self::get().inner.clone();
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();
        match arc.stream_metadata_map.entry(stream_id) {
            Entry::Occupied(mut occ) => {
                occ.get_mut().origin_trans = Some(origin_trans);
                occ.get_mut().register_ts = time;
                true
            }
            Entry::Vacant(vac) => {
                error!(
                    "stream_metadata_map: stream id dose not exist; {}",
                    vac.into_key()
                );
                false
            }
        }
    }
    pub fn init_media(media_config: MediaConfig) -> GlobalResult<u32> {
        let ssrc = media_config.ssrc;
        let time_schedule_key = TimeScheduleKey::RtpGateway(ssrc);
        let stream_id: Arc<str> = Arc::from(media_config.stream_id);
        let rtp_channel = RtpChannel::new(stream_id.clone());
        let converter = ConverterLayer::new(
            media_config.codec,
            media_config.filter,
            &media_config.output,
        );
        let output = OutputLayer::new(media_config.output.clone());
        let arc = Self::get().inner.clone();
        let in_wait_timeout = media_config
            .in_wait_timeout
            .unwrap_or_else(|| arc.stream_conf.in_wait_timeout);
        let out_idle_timeout = media_config
            .out_idle_timeout
            .unwrap_or_else(|| arc.stream_conf.out_idle_timeout);
        let metadata =
            StreamMetadata::new(ssrc, in_wait_timeout, out_idle_timeout, converter, output);
        let event = metadata.build_from_output_kind(
            media_config.output,
            ssrc,
            stream_id.clone(),
            arc.event_tx.clone(),
        );
        arc.time_schedule
            .insert(time_schedule_key, RTP_INPUT_CHECK_INTERVAL);
        arc.rtp_gateway_map.insert(ssrc, rtp_channel);
        arc.stream_metadata_map.insert(stream_id.clone(), metadata);
        //发送事件模拟触发消费输出流：如mp4录制;不监听output token,仅记录muxer资源;由rtp input超时清理资源
        if let Some(active_event) = event {
            let _ = arc
                .event_tx
                .try_send((Event::Active(active_event), None))
                .hand_log(|msg| error!("{msg}"));
        }
        Ok(ssrc)
    }
    pub fn init() -> GlobalResult<()> {
        if REGISTER.get().is_some() {
            return Ok(());
        }
        let server_conf = ServerConf::init_by_conf();
        let stream_conf = StreamConf::init_by_conf();
        let (event_tx, event_rx) = mpsc::channel(10000);
        let rt = GlobalRuntime::get_main_runtime();
        let _enter = rt.rt_handle.enter();
        let time_schedule = c100k::Cache::default();
        let inner = Inner {
            time_schedule,
            rtp_gateway_map: Default::default(),
            event_tx,
            stream_metadata_map: Default::default(),
            out_session_map: Default::default(),
            unknown_stream_map: Default::default(),
            user_token_map: Default::default(),
            server_conf,
            stream_conf,
        };
        let register = Register {
            inner: Arc::new(inner),
        };
        let arc = register.inner.clone();
        REGISTER
            .set(register)
            .map_err(|_| GlobalError::new_sys_error("Register already initialized", |_| {}))?;
        rt.rt_handle
            .spawn(event::schedule_event(arc, event_rx, rt.cancel.clone()));
        Ok(())
    }
}
//输出超时关闭：- 0 +
#[derive(Default, Debug)]
struct OutputCount {
    http_flv: AtomicU32,
    rtmp: AtomicU32,
    dash_fmp4: AtomicU32,
    dash_mp4: AtomicU32,
    hls_fmp4: AtomicU32,
    hls_ts: AtomicU32,
    rtsp: AtomicU32,
    gb28181_frame: AtomicU32,
    gb28181_ps: AtomicU32,
    web_rtc: AtomicU32,
    local_mp4: AtomicU32,
    local_ts: AtomicU32,
}
impl OutputCount {
    fn get_out_count(&self) -> u32 {
        self.http_flv.load(Ordering::Relaxed)
            + self.rtmp.load(Ordering::Relaxed)
            + self.dash_fmp4.load(Ordering::Relaxed)
            + self.dash_mp4.load(Ordering::Relaxed)
            + self.hls_fmp4.load(Ordering::Relaxed)
            + self.hls_ts.load(Ordering::Relaxed)
            + self.rtsp.load(Ordering::Relaxed)
            + self.gb28181_frame.load(Ordering::Relaxed)
            + self.gb28181_ps.load(Ordering::Relaxed)
            + self.web_rtc.load(Ordering::Relaxed)
            + self.local_mp4.load(Ordering::Relaxed)
            + self.local_ts.load(Ordering::Relaxed)
    }

    //获取复用器调用数量
    fn get_muxer_size(&self, output: OutputEnum) -> u32 {
        match output {
            OutputEnum::HttpFlv => self.http_flv.load(Ordering::Relaxed),
            OutputEnum::Rtmp => self.rtmp.load(Ordering::Relaxed),
            OutputEnum::DashFmp4 => self.dash_fmp4.load(Ordering::Relaxed),
            OutputEnum::HlsFmp4 => self.hls_fmp4.load(Ordering::Relaxed),
            OutputEnum::HlsTs => self.hls_ts.load(Ordering::Relaxed),
            OutputEnum::Rtsp => self.rtsp.load(Ordering::Relaxed),
            OutputEnum::Gb28181Frame => self.gb28181_frame.load(Ordering::Relaxed),
            OutputEnum::Gb28181Ps => self.gb28181_ps.load(Ordering::Relaxed),
            OutputEnum::WebRtc => self.web_rtc.load(Ordering::Relaxed),
            OutputEnum::LocalMp4 => self.local_mp4.load(Ordering::Relaxed),
            OutputEnum::LocalTs => self.local_ts.load(Ordering::Relaxed),
            OutputEnum::DashMp4 => self.dash_mp4.load(Ordering::Relaxed),
        }
    }

    //增加@OutputEnum点播数量，返回该output的当前点播数量
    fn add(&self, output: OutputEnum) -> u32 {
        (match output {
            OutputEnum::HttpFlv => self.http_flv.fetch_add(1, Ordering::Relaxed),
            OutputEnum::Rtmp => self.rtmp.fetch_add(1, Ordering::Relaxed),
            OutputEnum::DashFmp4 => self.dash_fmp4.fetch_add(1, Ordering::Relaxed),
            OutputEnum::HlsFmp4 => self.hls_fmp4.fetch_add(1, Ordering::Relaxed),
            OutputEnum::HlsTs => self.hls_ts.fetch_add(1, Ordering::Relaxed),
            OutputEnum::Rtsp => self.rtsp.fetch_add(1, Ordering::Relaxed),
            OutputEnum::Gb28181Frame => self.gb28181_frame.fetch_add(1, Ordering::Relaxed),
            OutputEnum::Gb28181Ps => self.gb28181_ps.fetch_add(1, Ordering::Relaxed),
            OutputEnum::WebRtc => self.web_rtc.fetch_add(1, Ordering::Relaxed),
            OutputEnum::LocalMp4 => self.local_mp4.fetch_add(1, Ordering::Relaxed),
            OutputEnum::LocalTs => self.local_ts.fetch_add(1, Ordering::Relaxed),
            OutputEnum::DashMp4 => self.dash_mp4.fetch_add(1, Ordering::Relaxed),
        }) + 1
    }
    //减少@OutputEnum点播数据，并判断该@OutputEnum对应的MuxerEnum是否已无输出使用
    //返回（@OutputEnum的点播数量、None在使用/Some无输出使用）
    //返回single_mux_play_count
    fn subtract(&self, output_enum: OutputEnum) -> u32 {
        match output_enum {
            OutputEnum::HttpFlv => {
                let last = self.http_flv.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.http_flv.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::Rtmp => {
                let last = self.rtmp.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.rtmp.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::DashFmp4 => {
                let last = self.dash_fmp4.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.dash_fmp4.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::HlsFmp4 => {
                let last = self.hls_fmp4.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.hls_fmp4.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::HlsTs => {
                let last = self.hls_ts.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.hls_ts.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::Rtsp => {
                let last = self.rtsp.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.rtsp.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::Gb28181Frame => {
                let last = self.gb28181_frame.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.gb28181_frame.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::Gb28181Ps => {
                let last = self.gb28181_ps.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.gb28181_ps.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::WebRtc => {
                let last = self.web_rtc.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.web_rtc.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::LocalMp4 => {
                let last = self.local_mp4.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.local_mp4.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::LocalTs => {
                let last = self.local_ts.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.local_ts.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
            OutputEnum::DashMp4 => {
                let last = self.dash_mp4.fetch_sub(1, Ordering::Relaxed);
                if last == 1 {
                    0
                } else if last == u32::MIN {
                    self.dash_mp4.store(u32::MIN, Ordering::Relaxed);
                    0
                } else {
                    last - 1
                }
            }
        }
    }
}

#[cfg(test)]
mod unknown_stream_tests {
    use super::{UNKNOWN_STREAM_COOLDOWN_MS, UNKNOWN_STREAM_EXPIRE_MS, UnknownStreamObservation};
    use std::net::SocketAddr;

    fn observation() -> UnknownStreamObservation {
        UnknownStreamObservation {
            ssrc: 200_000_001,
            remote_addr: "127.0.0.1:9000".parse::<SocketAddr>().unwrap(),
            protocol: "UDP".to_string(),
            first_seen_at_ms: 1_000,
            last_seen_at_ms: 1_000,
            packet_count: 1,
            last_notify_at_ms: 0,
        }
    }

    #[test]
    fn unknown_stream_requires_duration_and_packet_count() {
        let mut observation = observation();
        assert!(!observation.observe(1_500));
        assert!(observation.observe(3_000));
    }

    #[test]
    fn unknown_stream_notification_is_cooled_down() {
        let mut observation = observation();
        assert!(!observation.observe(1_500));
        assert!(observation.observe(3_000));
        assert!(!observation.observe(3_001));
        assert!(observation.observe(3_000 + UNKNOWN_STREAM_COOLDOWN_MS));
    }

    #[test]
    fn unknown_stream_observation_expires_after_idle_window() {
        let observation = observation();
        assert!(observation.idle_ms(1_000 + UNKNOWN_STREAM_EXPIRE_MS) >= UNKNOWN_STREAM_EXPIRE_MS);
    }
}
