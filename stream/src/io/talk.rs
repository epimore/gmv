use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::dashmap::mapref::entry::Entry;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::rw::{PacketWriter, U16BeLengthPrefixEncoder};
use base::net::state::{Association, Event, IoEventType, Protocol, Zip};
use base::once_cell::sync::Lazy;
use base::once_cell::sync::OnceCell;
use base::tokio::select;
use base::tokio::sync::mpsc;
use base::tokio::sync::mpsc::error::TrySendError;
use base::tokio::time::{self, Instant, MissedTickBehavior};
use base::tokio_util::sync::CancellationToken;
use gmv_domain::info::obj::{
    TALK_INPUT_PREFIX, TalkAnswerReq, TalkClosedEvent, TalkOpenReq, TalkOpenResp,
};
use parking_lot::Mutex;

use crate::general::cfg::StreamConf;
use crate::guard_integration::publish_guard_event;
use crate::io::http::call::{HttpClient, HttpSession, try_session_hook_rpc};
use crate::state::register::Register;

const TALK_INPUT_QUEUE_SIZE: usize = 32;
const TALK_JITTER_MIN_FRAMES: usize = 3;
const TALK_JITTER_MAX_FRAMES: usize = 8;
const RTP_HEADER_LEN: usize = 12;
const TALK_CLOSE_NOTIFY_RETRY: usize = 3;

static RTP_IO: OnceCell<RtpIo> = OnceCell::new();
static TALK_SESSIONS: Lazy<DashMap<String, TalkSession>> = Lazy::new(DashMap::new);

struct RtpIo {
    writer: PacketWriter<U16BeLengthPrefixEncoder>,
    output_tx: mpsc::Sender<Zip>,
    rtp_port: u16,
}

struct TalkSession {
    ssrc: u32,
    token: String,
    codec: String,
    sample_rate: u32,
    channel_count: u8,
    frame_duration_ms: u16,
    payload_type: Arc<AtomicU8>,
    target: Arc<Mutex<Option<TalkTarget>>>,
    input_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
}

#[derive(Clone, Copy)]
struct TalkTarget {
    addr: SocketAddr,
    protocol: Protocol,
}

pub struct TalkManager;

impl TalkManager {
    pub fn init_rtp_writer(
        writer: PacketWriter<U16BeLengthPrefixEncoder>,
        output_tx: mpsc::Sender<Zip>,
        rtp_port: u16,
    ) -> GlobalResult<()> {
        RTP_IO
            .set(RtpIo {
                writer,
                output_tx,
                rtp_port,
            })
            .map_err(|_| {
                GlobalError::new_biz_error(
                    BaseErrorCode::AlreadyExists.code(),
                    "rtp writer already initialized",
                    |msg| error!("{msg}"),
                )
            })
    }

    pub async fn open(req: TalkOpenReq) -> GlobalResult<TalkOpenResp> {
        validate_open_req(&req)?;

        let rtp_io = rtp_io()?;
        let rtp_port = rtp_io.rtp_port;
        let writer = rtp_io.writer.clone();
        let output_tx = rtp_io.output_tx.clone();
        let (input_tx, input_rx) = mpsc::channel(TALK_INPUT_QUEUE_SIZE);
        let payload_type = Arc::new(AtomicU8::new(req.payload_type));
        let target = Arc::new(Mutex::new(None));
        let cancel = CancellationToken::new();
        let input_timeout =
            Duration::from_secs(u64::from(StreamConf::init_by_conf().in_wait_timeout));

        let session = TalkSession {
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
        };

        match TALK_SESSIONS.entry(req.talk_id.clone()) {
            Entry::Occupied(_) => Err(GlobalError::new_biz_error(
                BaseErrorCode::AlreadyExists.code(),
                "talk session already exists",
                |msg| error!("{msg}: talk_id={}", req.talk_id),
            )),
            Entry::Vacant(vac) => {
                vac.insert(session);
                base::tokio::spawn(run_rtp_sender(
                    req.talk_id.clone(),
                    req.ssrc,
                    req.sample_rate,
                    req.frame_duration_ms,
                    input_timeout,
                    payload_type,
                    target,
                    writer,
                    output_tx,
                    input_rx,
                    cancel,
                ));
                Ok(TalkOpenResp {
                    talk_id: req.talk_id.clone(),
                    input_url: build_input_url(&req.talk_id),
                    rtp_port,
                    codec: req.codec,
                    sample_rate: req.sample_rate,
                    channel_count: req.channel_count,
                    payload_type: req.payload_type,
                    frame_duration_ms: req.frame_duration_ms,
                })
            }
        }
    }

    pub fn answer(req: TalkAnswerReq) -> GlobalResult<()> {
        let target = parse_device_addr(&req.device_ip, req.device_port)?;
        let protocol = parse_protocol(&req.protocol)?;
        match TALK_SESSIONS.get(&req.talk_id) {
            Some(session) => {
                *session.target.lock() = Some(TalkTarget {
                    addr: target,
                    protocol,
                });
                session
                    .payload_type
                    .store(req.payload_type, Ordering::Relaxed);
                info!(
                    "talk target ready: talk_id={}, ssrc={}, target={}, protocol={}, pt={}",
                    req.talk_id, session.ssrc, target, protocol, req.payload_type
                );
                Ok(())
            }
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "talk session not found",
                |msg| error!("{msg}: talk_id={}", req.talk_id),
            )),
        }
    }

    pub fn is_online(talk_id: &str) -> bool {
        TALK_SESSIONS.contains_key(talk_id)
    }

    pub fn close(talk_id: &str) -> bool {
        match TALK_SESSIONS.remove(talk_id) {
            Some((_, session)) => {
                if let Ok(rtp_io) = rtp_io() {
                    close_talk_target(
                        &rtp_io.output_tx,
                        "active_close",
                        current_target(&session.target),
                    );
                }
                session.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn check_token(talk_id: &str, token: &str) -> bool {
        TALK_SESSIONS
            .get(talk_id)
            .map(|session| session.token == token)
            .unwrap_or(false)
    }

    pub fn push_frame(talk_id: &str, frame: Vec<u8>) -> GlobalResult<()> {
        if frame.is_empty() {
            return Ok(());
        }
        match TALK_SESSIONS.get(talk_id) {
            Some(session) => match session.input_tx.try_send(frame) {
                Ok(_) => Ok(()),
                Err(TrySendError::Full(_)) => Err(GlobalError::new_biz_error(
                    BaseErrorCode::IoBusy.code(),
                    "talk input queue busy",
                    |msg| warn!("{msg}: talk_id={talk_id}"),
                )),
                Err(TrySendError::Closed(_)) => Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    "talk input queue closed",
                    |msg| warn!("{msg}: talk_id={talk_id}"),
                )),
            },
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "talk session not found",
                |msg| debug!("{msg}: talk_id={talk_id}"),
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

fn current_target(target: &Arc<Mutex<Option<TalkTarget>>>) -> Option<TalkTarget> {
    *target.lock()
}

fn close_talk_target(output_tx: &mpsc::Sender<Zip>, reason: &str, target: Option<TalkTarget>) {
    let Some(target) = target else {
        return;
    };
    if !matches!(target.protocol, Protocol::TCP) {
        return;
    }

    let association = Association::new(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        target.addr,
        Protocol::TCP,
    );
    let event = Event {
        association,
        type_code: IoEventType::Close,
    };
    match output_tx.try_send(Zip::build_event(event)) {
        Ok(_) => {
            info!(
                "talk tcp close event sent: target={}, reason={reason}",
                target.addr
            );
        }
        Err(TrySendError::Full(_)) => {
            warn!(
                "talk tcp close event dropped for full output channel: target={}, reason={reason}",
                target.addr
            );
        }
        Err(TrySendError::Closed(_)) => {
            warn!(
                "talk tcp close event dropped for closed output channel: target={}, reason={reason}",
                target.addr
            );
        }
    }
}

fn validate_open_req(req: &TalkOpenReq) -> GlobalResult<()> {
    if req.talk_id.is_empty() || req.token.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "talk_id/token must not be empty",
            |msg| error!("{msg}"),
        ));
    }
    if req.sample_rate == 0 || req.channel_count == 0 || req.frame_duration_ms == 0 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "invalid talk audio config",
            |msg| error!("{msg}: {:?}", req),
        ));
    }
    Ok(())
}

fn build_input_url(talk_id: &str) -> String {
    let mut proxy_addr = Register::get_server_conf()
        .proxy_addr
        .trim_end_matches('/')
        .to_string();
    if let Some(rest) = proxy_addr.strip_prefix("https://") {
        proxy_addr = format!("wss://{rest}");
    } else if let Some(rest) = proxy_addr.strip_prefix("http://") {
        proxy_addr = format!("ws://{rest}");
    }
    format!("{proxy_addr}{TALK_INPUT_PREFIX}/{talk_id}")
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
        "unsupported talk target protocol",
        |msg| error!("{msg}: protocol={protocol}"),
    ))
}

async fn run_rtp_sender(
    talk_id: String,
    ssrc: u32,
    sample_rate: u32,
    frame_duration_ms: u16,
    input_timeout: Duration,
    payload_type: Arc<AtomicU8>,
    target: Arc<Mutex<Option<TalkTarget>>>,
    writer: PacketWriter<U16BeLengthPrefixEncoder>,
    output_tx: mpsc::Sender<Zip>,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
) {
    let frame_samples = sample_rate.saturating_mul(frame_duration_ms as u32) / 1000;
    let mut ticker = time::interval(Duration::from_millis(frame_duration_ms as u64));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut queue = VecDeque::with_capacity(TALK_JITTER_MAX_FRAMES);
    let mut last_input = Instant::now();
    let mut ready = false;
    let mut first_packet = true;
    let mut seq = 0u16;
    let mut timestamp = 0u32;
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
                        if queue.len() >= TALK_JITTER_MAX_FRAMES {
                            queue.pop_front();
                        }
                        queue.push_back(frame);
                        if queue.len() >= TALK_JITTER_MIN_FRAMES {
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
                    warn!("talk input timeout: talk_id={talk_id}, ssrc={ssrc}");
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
                let packet = build_rtp_packet(ssrc, seq, timestamp, first_packet, pt, &frame);
                if let Err(err) = send_rtp_packet(&writer, Bytes::from(packet), target).await {
                    warn!(
                        "send talk rtp failed: talk_id={talk_id}, target={}, protocol={}, err={err}",
                        target.addr, target.protocol
                    );
                }
                seq = seq.wrapping_add(1);
                timestamp = timestamp.wrapping_add(frame_samples);
                first_packet = false;
            }
        }
    }

    if TALK_SESSIONS.remove(&talk_id).is_some() {
        close_talk_target(&output_tx, close_reason, current_target(&target));
        notify_talk_closed(&talk_id, close_reason).await;
    }
    info!("talk sender closed: talk_id={talk_id}, ssrc={ssrc}");
}

async fn send_rtp_packet(
    writer: &PacketWriter<U16BeLengthPrefixEncoder>,
    packet: Bytes,
    target: TalkTarget,
) -> GlobalResult<()> {
    match target.protocol {
        Protocol::UDP => writer.write_to(packet, target.addr, Protocol::UDP).await,
        Protocol::TCP => {
            if let Some(sink) = writer.tcp_sink(&target.addr) {
                return sink.write(packet).await;
            }
            let Some(sink) = writer.tcp_sink_by_ip(target.addr.ip()) else {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    "talk tcp sink is not available",
                    |msg| error!("{msg}: target={}", target.addr),
                ));
            };
            debug!(
                "talk tcp exact sink unavailable; fallback by ip: target={}",
                target.addr
            );
            sink.write(packet).await
        }
        Protocol::ALL => Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "protocol ALL cannot be used for talk",
            |msg| error!("{msg}"),
        )),
    }
}

async fn notify_talk_closed(talk_id: &str, reason: &str) {
    let event = TalkClosedEvent {
        talk_id: talk_id.to_string(),
        reason: reason.to_string(),
    };
    publish_guard_event("stream.talk_closed", format!("{event:?}").into_bytes());
    if let Some(response) = try_session_hook_rpc("stream.talk_closed", &event).await
        && response.error.is_none()
        && response.accepted
    {
        info!(
            "talk closed rpc accepted: talk_id={}, reason={}, resp={:?}",
            talk_id, reason, response
        );
        return;
    }
    for attempt in 1..=TALK_CLOSE_NOTIFY_RETRY {
        match HttpClient::template() {
            Ok(client) => match client.talk_closed(&event).await {
                Ok(resp) => {
                    let resp = resp.value();
                    if resp.code == 200 {
                        info!(
                            "talk closed notified: talk_id={}, reason={}, resp={:?}",
                            talk_id, reason, resp
                        );
                        return;
                    }
                    warn!(
                        "talk closed notify rejected: talk_id={}, reason={}, attempt={}, resp={:?}",
                        talk_id, reason, attempt, resp
                    );
                }
                Err(err) => {
                    warn!(
                        "talk closed notify failed: talk_id={}, reason={}, attempt={}, err={:?}",
                        talk_id, reason, attempt, err
                    );
                }
            },
            Err(err) => {
                warn!(
                    "talk closed notify client init failed: talk_id={}, reason={}, err={:?}",
                    talk_id, reason, err
                );
                return;
            }
        }
        time::sleep(Duration::from_secs(attempt as u64)).await;
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
