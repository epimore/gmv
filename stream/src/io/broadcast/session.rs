use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::rw::{PacketWriter, U16BeLengthPrefixEncoder};
use base::net::state::Protocol;
use base::once_cell::sync::Lazy;
use base::once_cell::sync::OnceCell;
use base::tokio::select;
use base::tokio::sync::Mutex as AsyncMutex;
use base::tokio::sync::Notify;
use base::tokio::sync::mpsc;
use base::tokio::sync::mpsc::error::TrySendError;
use base::tokio::time::{self, Instant, MissedTickBehavior};
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;
use gmv_domain::info::obj::{
    BROADCAST_INPUT_PREFIX, BroadcastClosedEvent, BroadcastConfigureLegReq, BroadcastOpenReq,
    BroadcastOpenResp,
};
use parking_lot::Mutex;

use crate::general::cfg::StreamConf;
use crate::guard_integration::{GuardEventPublish, publish_guard_event};
use crate::io::broadcast::packetizer::{
    DEFAULT_MAX_RTP_PAYLOAD_LEN, PsRtpPacketizer, PsRtpPacketizerConfig,
};
use crate::io::broadcast::{
    MAX_BROADCAST_LEGS_PER_NODE, MAX_BROADCAST_LEGS_PER_PARENT, MAX_BROADCAST_PARENTS_PER_NODE,
};
use crate::io::call::call_session_hook_rpc;
use crate::io::media_endpoint::{MediaEndpointLease, MediaEndpointManager, ReserveMediaEndpoint};
use crate::state::register::Register;

const BROADCAST_INPUT_QUEUE_SIZE: usize = 32;
const BROADCAST_JITTER_MIN_FRAMES: usize = 3;
const BROADCAST_JITTER_MAX_FRAMES: usize = 8;
const RTP_HEADER_LEN: usize = 12;

static RTP_IO: OnceCell<RtpIo> = OnceCell::new();
static BROADCAST_GENERATION: AtomicU64 = AtomicU64::new(1);
static BROADCAST_SETUP: Lazy<AsyncMutex<()>> = Lazy::new(|| AsyncMutex::new(()));
static BROADCAST_PARENTS: Lazy<DashMap<String, BroadcastParent>> = Lazy::new(DashMap::new);
static BROADCAST_SESSIONS: Lazy<DashMap<String, BroadcastSession>> = Lazy::new(DashMap::new);

struct RtpIo {
    runtime: GlobalRuntime,
    media_endpoints: Arc<MediaEndpointManager>,
}

struct BroadcastSession {
    parent_id: String,
    leg_id: String,
    ssrc: u32,
    token: String,
    codec: String,
    sample_rate: u32,
    channel_count: u8,
    frame_duration_ms: u16,
    payload_type: Arc<AtomicU8>,
    target: Arc<Mutex<Option<BroadcastTarget>>>,
    input_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    done: Arc<Notify>,
    session_hook_endpoint: Option<String>,
    endpoint: MediaEndpointLease,
}

struct BroadcastParent {
    generation: u64,
    token: String,
    codec: String,
    sample_rate: u32,
    channel_count: u8,
    frame_duration_ms: u16,
    input_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    legs: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone)]
struct BroadcastLegQueue {
    leg_id: String,
    input_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
}

#[derive(Clone, Copy)]
struct BroadcastTarget {
    addr: SocketAddr,
    protocol: Protocol,
    tcp_passive: bool,
    packetization: BroadcastPacketization,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BroadcastPacketization {
    RawG711,
    RtpPsG711,
}

pub struct BroadcastManager;

impl BroadcastManager {
    pub fn init(
        runtime: GlobalRuntime,
        media_endpoints: Arc<MediaEndpointManager>,
    ) -> GlobalResult<()> {
        RTP_IO
            .set(RtpIo {
                runtime,
                media_endpoints,
            })
            .map_err(|_| {
                GlobalError::new_biz_error(
                    BaseErrorCode::AlreadyExists.code(),
                    "rtp writer already initialized",
                    |msg| error!("{msg}"),
                )
            })
    }

    pub async fn open(req: BroadcastOpenReq) -> GlobalResult<BroadcastOpenResp> {
        validate_open_req(&req)?;
        let _setup_guard = BROADCAST_SETUP.lock().await;
        let rtp_io = rtp_io()?;
        let runtime = rtp_io.runtime.clone();
        let media_endpoints = rtp_io.media_endpoints.clone();
        let leg_id = leg_id(&req.leg_id, &req.broadcast_id);
        if let Some(session) = BROADCAST_SESSIONS.get(&leg_id) {
            validate_parent_contract(
                &req,
                &session.parent_id,
                &session.token,
                &session.codec,
                session.sample_rate,
                session.channel_count,
                session.frame_duration_ms,
            )?;
            return Ok(BroadcastOpenResp {
                broadcast_id: req.broadcast_id.clone(),
                leg_id,
                input_url: build_input_url(&req.broadcast_id),
                rtp_port: session.endpoint.port,
                codec: req.codec,
                sample_rate: req.sample_rate,
                channel_count: req.channel_count,
                payload_type: req.payload_type,
                frame_duration_ms: req.frame_duration_ms,
            });
        }

        let mut parent_created = false;
        if let Some(parent) = BROADCAST_PARENTS.get(&req.broadcast_id) {
            validate_parent_contract(
                &req,
                &req.broadcast_id,
                &parent.token,
                &parent.codec,
                parent.sample_rate,
                parent.channel_count,
                parent.frame_duration_ms,
            )?;
        } else {
            if BROADCAST_PARENTS.len() >= MAX_BROADCAST_PARENTS_PER_NODE {
                return Err(capacity_error("broadcast_parent_capacity_exceeded"));
            }
            let (parent_input_tx, parent_input_rx) = mpsc::channel(BROADCAST_INPUT_QUEUE_SIZE);
            let parent_cancel = runtime.cancel.child_token();
            let legs = Arc::new(Mutex::new(HashSet::new()));
            let generation = BROADCAST_GENERATION.fetch_add(1, Ordering::Relaxed);
            BROADCAST_PARENTS.insert(
                req.broadcast_id.clone(),
                BroadcastParent {
                    generation,
                    token: req.token.clone(),
                    codec: req.codec.clone(),
                    sample_rate: req.sample_rate,
                    channel_count: req.channel_count,
                    frame_duration_ms: req.frame_duration_ms,
                    input_tx: parent_input_tx,
                    cancel: parent_cancel.clone(),
                    legs: legs.clone(),
                },
            );
            if let Err(error) = runtime.spawn(
                "stream-broadcast-input",
                run_broadcast_input(
                    req.broadcast_id.clone(),
                    generation,
                    Duration::from_secs(u64::from(StreamConf::init_by_conf().in_wait_timeout)),
                    legs,
                    parent_input_rx,
                    parent_cancel,
                ),
            ) {
                BROADCAST_PARENTS.remove(&req.broadcast_id);
                return Err(error);
            }
            parent_created = true;
        }

        if BROADCAST_SESSIONS.len() >= MAX_BROADCAST_LEGS_PER_NODE {
            if parent_created {
                close_empty_parent(&req.broadcast_id);
            }
            return Err(capacity_error("broadcast_node_leg_capacity_exceeded"));
        }
        if BROADCAST_PARENTS
            .get(&req.broadcast_id)
            .map(|parent| parent.legs.lock().len() >= MAX_BROADCAST_LEGS_PER_PARENT)
            .unwrap_or(false)
        {
            if parent_created {
                close_empty_parent(&req.broadcast_id);
            }
            return Err(capacity_error("broadcast_parent_leg_capacity_exceeded"));
        }

        let leg_stream_id = if req.leg_stream_id.is_empty() {
            leg_id.clone()
        } else {
            req.leg_stream_id.clone()
        };
        let endpoint = match media_endpoints
            .reserve(ReserveMediaEndpoint {
                stream_id: leg_stream_id,
                lease_id: if req.lease_id.is_empty() {
                    format!("broadcast-{}-{leg_id}", req.broadcast_id)
                } else {
                    req.lease_id.clone()
                },
                route_id: if req.route_id.is_empty() {
                    format!("broadcast-{}-{leg_id}", req.broadcast_id)
                } else {
                    req.route_id.clone()
                },
                expected_ssrc: Some(req.ssrc),
                reservation_ttl: None,
                confirmed: true,
            })
            .await
        {
            Ok(endpoint) => endpoint,
            Err(error) => {
                if parent_created {
                    close_empty_parent(&req.broadcast_id);
                }
                return Err(error);
            }
        };
        let rtp_port = endpoint.port;
        let writer = endpoint.writer.clone();
        let (input_tx, input_rx) = mpsc::channel(BROADCAST_JITTER_MAX_FRAMES);
        let payload_type = Arc::new(AtomicU8::new(req.payload_type));
        let target = Arc::new(Mutex::new(None));
        let cancel = runtime.cancel.child_token();
        let done = Arc::new(Notify::new());
        let input_timeout =
            Duration::from_secs(u64::from(StreamConf::init_by_conf().in_wait_timeout));

        let session = BroadcastSession {
            parent_id: req.broadcast_id.clone(),
            leg_id: leg_id.clone(),
            ssrc: req.ssrc,
            token: req.token.clone(),
            codec: req.codec.clone(),
            sample_rate: req.sample_rate,
            channel_count: req.channel_count,
            frame_duration_ms: req.frame_duration_ms,
            payload_type: payload_type.clone(),
            target: target.clone(),
            input_tx,
            cancel: cancel.clone(),
            done: done.clone(),
            session_hook_endpoint: req.session_hook_endpoint.clone(),
            endpoint: endpoint.clone(),
        };
        BROADCAST_SESSIONS.insert(leg_id.clone(), session);
        if let Some(parent) = BROADCAST_PARENTS.get(&req.broadcast_id) {
            parent.legs.lock().insert(leg_id.clone());
        }
        let cleanup_media_endpoints = media_endpoints.clone();
        let cleanup_endpoint = endpoint.clone();
        if let Err(err) = runtime.spawn(
            "stream-broadcast-leg-sender",
            run_rtp_sender(
                req.broadcast_id.clone(),
                leg_id.clone(),
                req.ssrc,
                req.sample_rate,
                req.frame_duration_ms,
                input_timeout,
                payload_type,
                target,
                writer,
                media_endpoints,
                endpoint,
                input_rx,
                cancel,
                done,
            ),
        ) {
            BROADCAST_SESSIONS.remove(&leg_id);
            remove_parent_leg(&req.broadcast_id, &leg_id);
            if parent_created {
                close_empty_parent(&req.broadcast_id);
            }
            if let Err(close_error) = cleanup_media_endpoints
                .release(&cleanup_endpoint.stream_id, &cleanup_endpoint.lease_id)
                .await
            {
                error!(
                    "broadcast endpoint rollback failed: broadcast_id={}, leg_id={leg_id}, error={close_error}",
                    req.broadcast_id
                );
            }
            return Err(err);
        }
        Ok(BroadcastOpenResp {
            broadcast_id: req.broadcast_id.clone(),
            leg_id,
            input_url: build_input_url(&req.broadcast_id),
            rtp_port,
            codec: req.codec,
            sample_rate: req.sample_rate,
            channel_count: req.channel_count,
            payload_type: req.payload_type,
            frame_duration_ms: req.frame_duration_ms,
        })
    }

    pub async fn configure_leg(req: BroadcastConfigureLegReq) -> GlobalResult<()> {
        let leg_id = leg_id(&req.leg_id, &req.broadcast_id);
        let target = parse_device_addr(&req.device_ip, req.device_port)?;
        let protocol = parse_protocol(&req.protocol)?;
        let transport = req.transport.trim().to_ascii_lowercase();
        let expected_protocol = match transport.as_str() {
            "udp" => Protocol::UDP,
            "tcp_active" | "tcp_passive" => Protocol::TCP,
            _ => {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidRequest.code(),
                    "invalid_media_transport",
                    |msg| error!("{msg}: transport={}", req.transport),
                ));
            }
        };
        if protocol != expected_protocol {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "broadcast_transport_mismatch",
                |msg| {
                    error!(
                        "{msg}: transport={}, protocol={}",
                        req.transport, req.protocol
                    )
                },
            ));
        }
        let packetization = match req.packetization.as_str() {
            "raw_g711" if req.inner_codec == "PCMA" && req.rtp_clock_rate == 8_000 => {
                BroadcastPacketization::RawG711
            }
            "rtp_ps_g711" if req.inner_codec == "PCMA" && req.rtp_clock_rate == 90_000 => {
                BroadcastPacketization::RtpPsG711
            }
            _ => {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::Unsupported.code(),
                    "broadcast_profile_unsupported",
                    |msg| {
                        error!(
                            "{msg}: packetization={}, inner_codec={}, clock_rate={}",
                            req.packetization, req.inner_codec, req.rtp_clock_rate
                        )
                    },
                ));
            }
        };
        let endpoint = BROADCAST_SESSIONS
            .get(&leg_id)
            .map(|session| session.endpoint.clone())
            .ok_or_else(|| {
                GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "broadcast session not found",
                    |msg| error!("{msg}: broadcast_id={}", req.broadcast_id),
                )
            })?;
        if transport == "tcp_active" {
            rtp_io()?
                .media_endpoints
                .connect_tcp_active(crate::io::media_endpoint::ConnectMediaEndpoint {
                    stream_id: endpoint.stream_id.clone(),
                    lease_id: endpoint.lease_id.clone(),
                    route_id: endpoint.route_id.clone(),
                    endpoint_id: endpoint.endpoint_id.clone(),
                    generation: endpoint.generation,
                    remote_addr: target,
                    local_addr: None,
                    timeout: Duration::from_secs(5),
                })
                .await?;
        }
        match BROADCAST_SESSIONS.get(&leg_id) {
            Some(session) => {
                if session.endpoint.endpoint_id != endpoint.endpoint_id
                    || session.endpoint.generation != endpoint.generation
                {
                    return Err(GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "stale_endpoint_generation",
                        |msg| {
                            debug!(
                                "{msg}: action=set_broadcast_target, outcome=ignored, reason=late_completion, broadcast_id={}, leg_id={leg_id}",
                                req.broadcast_id
                            )
                        },
                    ));
                }
                *session.target.lock() = Some(BroadcastTarget {
                    addr: target,
                    protocol,
                    tcp_passive: transport == "tcp_passive",
                    packetization,
                });
                session
                    .payload_type
                    .store(req.payload_type, Ordering::Relaxed);
                info!(
                    "broadcast target ready: broadcast_id={}, leg_id={}, ssrc={}, target={}, protocol={}, packetization={packetization:?}, pt={}",
                    req.broadcast_id,
                    session.leg_id,
                    session.ssrc,
                    target,
                    protocol,
                    req.payload_type
                );
                Ok(())
            }
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "broadcast session not found",
                |msg| error!("{msg}: broadcast_id={}", req.broadcast_id),
            )),
        }
    }

    pub fn is_online(broadcast_id: &str) -> bool {
        BROADCAST_PARENTS.contains_key(broadcast_id)
    }

    pub async fn wait_ready(
        broadcast_id: &str,
        requested_leg_id: &str,
        timeout: Duration,
    ) -> GlobalResult<bool> {
        let leg_id = leg_id(requested_leg_id, broadcast_id);
        let (writer, target) = BROADCAST_SESSIONS
            .get(&leg_id)
            .map(|session| {
                (
                    session.endpoint.writer.clone(),
                    current_target(&session.target),
                )
            })
            .ok_or_else(|| {
                GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "broadcast session not found",
                    |msg| debug!("{msg}: broadcast_id={broadcast_id}"),
                )
            })?;
        let Some(target) = target else {
            return Ok(false);
        };
        match target.protocol {
            Protocol::UDP => Ok(true),
            Protocol::TCP if target.tcp_passive => {
                writer.wait_tcp_sink(target.addr, timeout).await?;
                Ok(true)
            }
            Protocol::TCP => Ok(writer.tcp_sink(&target.addr).is_some()),
            Protocol::ALL => Ok(false),
        }
    }

    pub fn active_session_count() -> usize {
        BROADCAST_PARENTS.len()
    }

    pub async fn close(broadcast_id: &str, requested_leg_id: &str) -> GlobalResult<bool> {
        if !requested_leg_id.is_empty() {
            return close_leg(broadcast_id, requested_leg_id, "leg_close").await;
        }
        let Some((_, parent)) = BROADCAST_PARENTS.remove(broadcast_id) else {
            return Ok(false);
        };
        parent.cancel.cancel();
        let leg_ids = parent.legs.lock().iter().cloned().collect::<Vec<_>>();
        let mut closed = false;
        for leg_id in leg_ids {
            closed |= close_leg(broadcast_id, &leg_id, "parent_close").await?;
        }
        Ok(closed || parent.legs.lock().is_empty())
    }

    pub fn check_token(broadcast_id: &str, token: &str) -> bool {
        BROADCAST_PARENTS
            .get(broadcast_id)
            .map(|parent| parent.token == token)
            .unwrap_or(false)
    }

    pub fn push_frame(broadcast_id: &str, frame: Vec<u8>) -> GlobalResult<()> {
        if frame.is_empty() {
            return Ok(());
        }
        match BROADCAST_PARENTS.get(broadcast_id) {
            Some(parent) => match parent.input_tx.try_send(frame) {
                Ok(_) => Ok(()),
                Err(TrySendError::Full(_)) => Err(GlobalError::new_biz_error(
                    BaseErrorCode::IoBusy.code(),
                    "broadcast input queue busy",
                    |msg| warn!("{msg}: broadcast_id={broadcast_id}"),
                )),
                Err(TrySendError::Closed(_)) => Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    "broadcast input queue closed",
                    |msg| warn!("{msg}: broadcast_id={broadcast_id}"),
                )),
            },
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "broadcast session not found",
                |msg| debug!("{msg}: broadcast_id={broadcast_id}"),
            )),
        }
    }
}

fn rtp_io() -> GlobalResult<&'static RtpIo> {
    RTP_IO.get().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "rtp writer is not initialized",
            |msg| error!("{msg}"),
        )
    })
}

fn leg_id(requested_leg_id: &str, broadcast_id: &str) -> String {
    if requested_leg_id.is_empty() {
        broadcast_id.to_string()
    } else {
        requested_leg_id.to_string()
    }
}

fn capacity_error(reason: &'static str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::IoBusy.code(), reason, |msg| warn!("{msg}"))
}

fn validate_parent_contract(
    req: &BroadcastOpenReq,
    parent_id: &str,
    token: &str,
    codec: &str,
    sample_rate: u32,
    channel_count: u8,
    frame_duration_ms: u16,
) -> GlobalResult<()> {
    if parent_id == req.broadcast_id
        && token == req.token
        && codec == req.codec
        && sample_rate == req.sample_rate
        && channel_count == req.channel_count
        && frame_duration_ms == req.frame_duration_ms
    {
        return Ok(());
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "broadcast_parent_contract_mismatch",
        |msg| error!("{msg}: broadcast_id={}", req.broadcast_id),
    ))
}

fn remove_parent_leg(broadcast_id: &str, leg_id: &str) -> bool {
    let Some(parent) = BROADCAST_PARENTS.get(broadcast_id) else {
        return true;
    };
    let mut legs = parent.legs.lock();
    legs.remove(leg_id);
    legs.is_empty()
}

fn close_empty_parent(broadcast_id: &str) {
    let Some(parent) = BROADCAST_PARENTS.get(broadcast_id) else {
        return;
    };
    if !parent.legs.lock().is_empty() {
        return;
    }
    let generation = parent.generation;
    parent.cancel.cancel();
    drop(parent);
    BROADCAST_PARENTS.remove_if(broadcast_id, |_, parent| parent.generation == generation);
}

async fn close_leg(broadcast_id: &str, leg_id: &str, reason: &str) -> GlobalResult<bool> {
    let Some((_, session)) = BROADCAST_SESSIONS.remove(leg_id) else {
        return Ok(false);
    };
    if session.parent_id != broadcast_id {
        BROADCAST_SESSIONS.insert(leg_id.to_string(), session);
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "broadcast_leg_parent_mismatch",
            |msg| error!("{msg}: broadcast_id={broadcast_id}, leg_id={leg_id}"),
        ));
    }
    close_broadcast_target(
        &session.endpoint.writer,
        reason,
        current_target(&session.target),
    );
    session.cancel.cancel();
    time::timeout(Duration::from_secs(5), session.done.notified())
        .await
        .map_err(|_| {
            GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "broadcast_leg_close_timeout",
                |msg| error!("{msg}: broadcast_id={broadcast_id}, leg_id={leg_id}"),
            )
        })?;
    rtp_io()?
        .media_endpoints
        .release(&session.endpoint.stream_id, &session.endpoint.lease_id)
        .await?;
    if remove_parent_leg(broadcast_id, leg_id) {
        close_empty_parent(broadcast_id);
    }
    Ok(true)
}

async fn run_broadcast_input(
    broadcast_id: String,
    generation: u64,
    input_timeout: Duration,
    legs: Arc<Mutex<HashSet<String>>>,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
) {
    let mut last_input = Instant::now();
    let mut ticker = time::interval(Duration::from_millis(250));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let close_reason = loop {
        select! {
            _ = cancel.cancelled() => break "cancelled",
            item = input_rx.recv() => match item {
                Some(frame) => {
                    last_input = Instant::now();
                    let leg_ids = legs.lock().iter().cloned().collect::<Vec<_>>();
                    let leg_queues = leg_ids
                        .into_iter()
                        .filter_map(|leg_id| {
                            BROADCAST_SESSIONS.get(&leg_id).map(|session| BroadcastLegQueue {
                                leg_id,
                                input_tx: session.input_tx.clone(),
                                cancel: session.cancel.clone(),
                            })
                        })
                        .collect::<Vec<_>>();
                    fan_out_frame(&broadcast_id, &leg_queues, &frame);
                }
                None => break "input_closed",
            },
            _ = ticker.tick() => {
                if last_input.elapsed() > input_timeout {
                    break "input_timeout";
                }
            }
        }
    };
    if let Some((_, parent)) =
        BROADCAST_PARENTS.remove_if(&broadcast_id, |_, parent| parent.generation == generation)
    {
        let leg_ids = parent.legs.lock().iter().cloned().collect::<Vec<_>>();
        for leg_id in leg_ids {
            if let Err(error) = close_leg(&broadcast_id, &leg_id, close_reason).await {
                error!(
                    "broadcast leg cleanup failed: broadcast_id={broadcast_id}, leg_id={leg_id}, error={error}"
                );
            }
        }
    }
    info!(
        "broadcast input closed: broadcast_id={broadcast_id}, generation={generation}, reason={close_reason}"
    );
}

fn fan_out_frame(broadcast_id: &str, legs: &[BroadcastLegQueue], frame: &[u8]) {
    for leg in legs {
        match leg.input_tx.try_send(frame.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!(
                    "broadcast leg queue busy: broadcast_id={broadcast_id}, leg_id={}",
                    leg.leg_id
                );
                leg.cancel.cancel();
            }
            Err(TrySendError::Closed(_)) => leg.cancel.cancel(),
        }
    }
}

fn current_target(target: &Arc<Mutex<Option<BroadcastTarget>>>) -> Option<BroadcastTarget> {
    *target.lock()
}

fn close_broadcast_target(
    writer: &PacketWriter<U16BeLengthPrefixEncoder>,
    reason: &str,
    target: Option<BroadcastTarget>,
) {
    let Some(target) = target else {
        return;
    };
    if !matches!(target.protocol, Protocol::TCP) {
        return;
    }

    writer.remove_tcp_writer(&target.addr);
    info!(
        "broadcast tcp association closed: target={}, reason={reason}",
        target.addr
    );
}

fn validate_open_req(req: &BroadcastOpenReq) -> GlobalResult<()> {
    if req.broadcast_id.is_empty() || req.token.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "broadcast_id/token must not be empty",
            |msg| error!("{msg}"),
        ));
    }
    if req.sample_rate == 0 || req.channel_count == 0 || req.frame_duration_ms == 0 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "invalid broadcast audio config",
            |msg| error!("{msg}: {:?}", req),
        ));
    }
    Ok(())
}

fn build_input_url(broadcast_id: &str) -> String {
    let mut proxy_addr = Register::get_server_conf()
        .http
        .public_url
        .trim_end_matches('/')
        .to_string();
    if let Some(rest) = proxy_addr.strip_prefix("https://") {
        proxy_addr = format!("wss://{rest}");
    } else if let Some(rest) = proxy_addr.strip_prefix("http://") {
        proxy_addr = format!("ws://{rest}");
    }
    format!("{proxy_addr}{BROADCAST_INPUT_PREFIX}/{broadcast_id}")
}

fn parse_device_addr(ip: &str, port: u16) -> GlobalResult<SocketAddr> {
    format!("{ip}:{port}")
        .parse::<SocketAddr>()
        .hand_log(|msg| error!("{msg}: ip={ip}, port={port}"))
}

fn parse_protocol(protocol: &str) -> GlobalResult<Protocol> {
    let compact = protocol
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    if compact.contains("TCP") {
        return Ok(Protocol::TCP);
    }
    if compact.contains("UDP") || compact == "RTPAVP" {
        return Ok(Protocol::UDP);
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidRequest.code(),
        "unsupported broadcast target protocol",
        |msg| error!("{msg}: protocol={protocol}"),
    ))
}

async fn run_rtp_sender(
    broadcast_id: String,
    leg_id: String,
    ssrc: u32,
    sample_rate: u32,
    frame_duration_ms: u16,
    input_timeout: Duration,
    payload_type: Arc<AtomicU8>,
    target: Arc<Mutex<Option<BroadcastTarget>>>,
    writer: PacketWriter<U16BeLengthPrefixEncoder>,
    media_endpoints: Arc<MediaEndpointManager>,
    endpoint: MediaEndpointLease,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
    done: Arc<Notify>,
) {
    let frame_samples = sample_rate.saturating_mul(frame_duration_ms as u32) / 1000;
    let mut ticker = time::interval(Duration::from_millis(frame_duration_ms as u64));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut queue = VecDeque::with_capacity(BROADCAST_JITTER_MAX_FRAMES);
    let mut last_input = Instant::now();
    let mut ready = false;
    let mut first_packet = true;
    let mut seq = 0u16;
    let mut timestamp = 0u32;
    let mut ps_packetizer: Option<PsRtpPacketizer> = None;
    let mut close_reason = "closed";

    loop {
        select! {
            _ = cancel.cancelled() => {
                close_reason = "cancelled";
                break;
            }
            item = input_rx.recv() => {
                match item {
                    Some(frame) => {
                        last_input = Instant::now();
                        if queue.len() >= BROADCAST_JITTER_MAX_FRAMES {
                            queue.pop_front();
                        }
                        queue.push_back(frame);
                        if queue.len() >= BROADCAST_JITTER_MIN_FRAMES {
                            ready = true;
                        }
                    }
                    None => {
                        close_reason = "input_closed";
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if last_input.elapsed() > input_timeout {
                    warn!("broadcast input timeout: broadcast_id={broadcast_id}, leg_id={leg_id}, ssrc={ssrc}");
                    close_reason = "input_timeout";
                    break;
                }
                if !ready {
                    continue;
                }
                let Some(frame) = queue.pop_front() else {
                    continue;
                };
                let target = { *target.lock() };
                let Some(target) = target else {
                    continue;
                };
                let pt = payload_type.load(Ordering::Relaxed);
                let packets = match target.packetization {
                    BroadcastPacketization::RawG711 => {
                        let packet = build_rtp_packet(ssrc, seq, timestamp, first_packet, pt, &frame);
                        seq = seq.wrapping_add(1);
                        timestamp = timestamp.wrapping_add(frame_samples);
                        vec![packet]
                    }
                    BroadcastPacketization::RtpPsG711 => {
                        let packetizer = match ps_packetizer.as_mut() {
                            Some(packetizer) => packetizer,
                            None => {
                                let packetizer = match PsRtpPacketizer::new(PsRtpPacketizerConfig {
                                    payload_type: pt,
                                    ssrc,
                                    sequence: seq,
                                    timestamp,
                                    frame_duration_ms,
                                    max_rtp_payload_len: DEFAULT_MAX_RTP_PAYLOAD_LEN,
                                }) {
                                    Ok(packetizer) => packetizer,
                                    Err(err) => {
                                        warn!("broadcast PS packetizer init failed: broadcast_id={broadcast_id}, leg_id={leg_id}, err={err}");
                                        close_reason = "ps_packetization_failed";
                                        break;
                                    }
                                };
                                ps_packetizer.insert(packetizer)
                            }
                        };
                        match packetizer.packetize(&frame) {
                            Ok(packets) => packets.into_iter().map(|packet| packet.bytes).collect(),
                            Err(err) => {
                                warn!("broadcast PS packetization failed: broadcast_id={broadcast_id}, leg_id={leg_id}, err={err}");
                                close_reason = "ps_packetization_failed";
                                break;
                            }
                        }
                    }
                };
                let mut send_failed = false;
                for packet in packets {
                    if let Err(err) = send_rtp_packet(&writer, Bytes::from(packet), target).await {
                        warn!(
                            "send broadcast rtp failed: broadcast_id={broadcast_id}, leg_id={leg_id}, target={}, protocol={}, err={err}",
                            target.addr, target.protocol
                        );
                        close_reason = "output_error";
                        send_failed = true;
                        break;
                    }
                }
                if send_failed {
                    break;
                }
                first_packet = false;
            }
        }
    }

    if let Some((_, session)) = BROADCAST_SESSIONS.remove(&leg_id) {
        close_broadcast_target(&writer, close_reason, current_target(&target));
        remove_parent_leg(&broadcast_id, &leg_id);
        notify_broadcast_closed(
            &broadcast_id,
            &leg_id,
            close_reason,
            session.session_hook_endpoint,
        )
        .await;
        close_empty_parent(&broadcast_id);
    }
    if let Err(error) = media_endpoints
        .release(&endpoint.stream_id, &endpoint.lease_id)
        .await
    {
        error!(
            "broadcast media endpoint release failed: broadcast_id={broadcast_id}, leg_id={leg_id}, error={error}"
        );
    }
    info!("broadcast sender closed: broadcast_id={broadcast_id}, leg_id={leg_id}, ssrc={ssrc}");
    done.notify_one();
}

async fn send_rtp_packet(
    writer: &PacketWriter<U16BeLengthPrefixEncoder>,
    packet: Bytes,
    target: BroadcastTarget,
) -> GlobalResult<()> {
    match target.protocol {
        Protocol::UDP => writer.write_to(packet, target.addr, Protocol::UDP).await,
        Protocol::TCP => {
            if let Some(sink) = writer.tcp_sink(&target.addr) {
                return sink.write(packet).await;
            }
            Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "tcp_peer_not_connected",
                |msg| error!("{msg}: target={}", target.addr),
            ))
        }
        Protocol::ALL => Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "protocol ALL cannot be used for broadcast",
            |msg| error!("{msg}"),
        )),
    }
}

async fn notify_broadcast_closed(
    broadcast_id: &str,
    leg_id: &str,
    reason: &str,
    session_hook_endpoint: Option<String>,
) {
    let event = BroadcastClosedEvent {
        broadcast_id: broadcast_id.to_string(),
        leg_id: leg_id.to_string(),
        reason: reason.to_string(),
    };
    if let Some(endpoint) = session_hook_endpoint.as_deref() {
        if let Some(response) =
            call_session_hook_rpc(endpoint, "stream.broadcast_closed", &event).await
        {
            if response.accepted {
                info!(
                    "broadcast closed event sent to session: broadcast_id={}, leg_id={}, reason={}",
                    broadcast_id, leg_id, reason
                );
                return;
            }
            warn!(
                "broadcast closed session hook rejected: broadcast_id={}, leg_id={}, reason={}, error={:?}",
                broadcast_id, leg_id, reason, response.error
            );
        }
    } else {
        warn!(
            "broadcast closed session hook endpoint missing: broadcast_id={broadcast_id}, leg_id={leg_id}"
        );
    }
    let published = publish_guard_event(
        "stream.broadcast_closed.fallback",
        format!("{event:?}").into_bytes(),
    );
    if published == GuardEventPublish::Queued {
        info!(
            "broadcast closed using guard fallback: broadcast_id={}, leg_id={}, reason={}",
            broadcast_id, leg_id, reason
        );
    }
}

fn build_rtp_packet(
    ssrc: u32,
    seq: u16,
    timestamp: u32,
    marker: bool,
    payload_type: u8,
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(RTP_HEADER_LEN + payload.len());
    out.push(0x80);
    out.push((payload_type & 0x7f) | if marker { 0x80 } else { 0 });
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&timestamp.to_be_bytes());
    out.extend_from_slice(&ssrc.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_leg_is_cancelled_without_blocking_healthy_leg() {
        let (slow_tx, _slow_rx) = mpsc::channel(1);
        slow_tx.try_send(vec![0]).expect("fill slow queue");
        let slow_cancel = CancellationToken::new();
        let (fast_tx, mut fast_rx) = mpsc::channel(1);
        let fast_cancel = CancellationToken::new();
        let legs = vec![
            BroadcastLegQueue {
                leg_id: "slow".to_string(),
                input_tx: slow_tx,
                cancel: slow_cancel.clone(),
            },
            BroadcastLegQueue {
                leg_id: "fast".to_string(),
                input_tx: fast_tx,
                cancel: fast_cancel.clone(),
            },
        ];

        fan_out_frame("parent", &legs, &[1, 2, 3]);

        assert!(slow_cancel.is_cancelled());
        assert!(!fast_cancel.is_cancelled());
        assert_eq!(
            fast_rx.try_recv().expect("healthy leg frame"),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn broadcast_leg_id_falls_back_to_parent_for_single_target_compatibility() {
        assert_eq!(leg_id("", "parent"), "parent");
        assert_eq!(leg_id("leg-1", "parent"), "leg-1");
    }
}
