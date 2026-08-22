use crate::io::http::out::{OutPlayKind, stream_user_token_authorize};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::hlsfmp4::HLS_PART_TARGET_US;
use crate::media::context::format::muxer::MuxerEnum;
use crate::media::context::format::{HlsPart as HlsPartMeta, MuxPacket};
use crate::state::register::Register;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::once_cell::sync::Lazy;
use base::tokio::sync::{Notify, RwLock, broadcast, oneshot};
use gmv_domain::info::output::OutputEnum;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HLS_WINDOW_SIZE: usize = 6;
const HLS_PART_WINDOW_SEGMENTS: usize = 3;
const HLS_TARGET_DURATION: u64 = 4;
const HLS_MIN_PLAYLIST_DURATION_US: u64 = HLS_TARGET_DURATION * 3 * 1_000_000;
const HLS_PART_HOLD_BACK_SECONDS: f64 = 1.5;
const HLS_BLOCK_RELOAD_TIMEOUT: Duration = Duration::from_secs(HLS_TARGET_DURATION * 3);
const HLS_PART_BLOCK_TIMEOUT: Duration = Duration::from_secs(3);
const HLS_MEDIA_RETENTION: Duration = Duration::from_secs(30);

static HLS_STORES: Lazy<DashMap<u32, Arc<HlsStoreHandle>>> = Lazy::new(DashMap::new);

#[derive(Clone)]
struct HlsPart {
    segment_seq: usize,
    part_seq: usize,
    data: Bytes,
    duration_us: u64,
    independent: bool,
}

#[derive(Clone)]
struct HlsSegment {
    seq: usize,
    data: Bytes,
    duration_us: u64,
    parts: Vec<HlsPart>,
    independent: bool,
    discontinuity: bool,
}

struct PendingSegment {
    seq: usize,
    data: Vec<u8>,
    duration_us: u64,
    parts: Vec<HlsPart>,
    discontinuity: bool,
}

struct RetiredMedia {
    segment_seq: usize,
    segment: Option<Bytes>,
    parts: Vec<HlsPart>,
    expires_at: Instant,
}

struct RetiredInit {
    generation: u64,
    data: Bytes,
    expires_at: Instant,
}

struct HlsStore {
    init: Bytes,
    segments: VecDeque<HlsSegment>,
    pending: Option<PendingSegment>,
    retired: VecDeque<RetiredMedia>,
    retired_inits: VecDeque<RetiredInit>,
    epoch: Option<Instant>,
    init_generation: u64,
    discontinuity_sequence: u64,
    next_discontinuity: bool,
    ended: bool,
}

struct HlsStoreHandle {
    channel_id: u64,
    state: RwLock<HlsStore>,
    notify: Notify,
}

#[derive(Clone, Copy, Default)]
struct DeliveryDirectives {
    msn: Option<usize>,
    part: Option<usize>,
}

enum MediaResource {
    Segment(usize),
    Part { segment_seq: usize, part_seq: usize },
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PlaylistProfile {
    Standard,
    LowLatency,
}

pub async fn m3u8_handler(
    stream_id: Arc<str>,
    token: Arc<str>,
    addr: SocketAddr,
    query: &HashMap<String, String>,
    profile: PlaylistProfile,
) -> Response<Body> {
    let directives = match profile {
        PlaylistProfile::Standard => DeliveryDirectives::default(),
        PlaylistProfile::LowLatency => match parse_delivery_directives(query) {
            Ok(directives) => directives,
            Err(()) => return status_response(StatusCode::BAD_REQUEST),
        },
    };
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id.clone()) else {
        return res_404();
    };
    let ssrc = base.rtp_info.ssrc;
    match stream_user_token_authorize(
        OutputEnum::HlsFmp4,
        base,
        stream_id.clone(),
        token.clone(),
        addr,
    )
    .await
    {
        OutPlayKind::Forbid => return res_401(),
        OutPlayKind::Notfound => return res_404(),
        OutPlayKind::Play => {}
    }
    let store = match ensure_store(&stream_id, ssrc).await {
        Some(store) => store,
        None => return res_404(),
    };
    let directives_valid = {
        let state = store.state.read().await;
        delivery_directives_valid(&state, directives)
    };
    if !directives_valid {
        return status_response(StatusCode::BAD_REQUEST);
    }
    if Register::insert_out_token(stream_id.clone(), OutputEnum::HlsFmp4, token.clone()).is_err() {
        return res_404();
    }
    if profile == PlaylistProfile::LowLatency && !wait_for_playlist(&store, directives).await {
        return status_response(StatusCode::SERVICE_UNAVAILABLE);
    }

    let state = store.state.read().await;
    let encoded_token: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    let playlist = render_playlist(&state, &stream_id, &encoded_token, profile);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-cache, no-store")
        .body(Body::from(playlist))
        .expect("valid HLS playlist response")
}

pub async fn init_mp4_handler(
    stream_id: Arc<str>,
    token: Arc<str>,
    query: &HashMap<String, String>,
) -> Response<Body> {
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id.clone()) else {
        return res_404();
    };
    if !Register::check_token(&(token, Arc::from(base.stream_id.as_str()))) {
        return res_401();
    }
    let Some(store) = ensure_store(&stream_id, base.rtp_info.ssrc).await else {
        return res_404();
    };
    let state = store.state.read().await;
    let init = match query.get("gmv-hls-generation") {
        Some(value) => match value
            .parse::<u64>()
            .ok()
            .and_then(|generation| find_init(&state, generation))
        {
            Some(init) => init.clone(),
            None => return res_404(),
        },
        None => state.init.clone(),
    };
    media_response(init)
}

pub async fn segment_mp4_handler(segment_id: Arc<str>, token: Arc<str>) -> Response<Body> {
    let Some((stream_id, resource)) = parse_media_resource(&segment_id) else {
        return res_404();
    };
    let stream_id: Arc<str> = Arc::from(stream_id);
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id.clone()) else {
        return res_404();
    };
    if !Register::check_token(&(token, stream_id.clone())) {
        return res_401();
    }
    let Some(store) = ensure_store(&stream_id, base.rtp_info.ssrc).await else {
        return res_404();
    };
    if let MediaResource::Part {
        segment_seq,
        part_seq,
    } = resource
    {
        wait_for_part(&store, segment_seq, part_seq).await;
    }
    let state = store.state.read().await;
    match resource {
        MediaResource::Segment(seq) => find_segment(&state, seq)
            .map(|data| media_response(data.clone()))
            .unwrap_or_else(res_404),
        MediaResource::Part {
            segment_seq,
            part_seq,
        } => find_part(&state, segment_seq, part_seq)
            .map(|part| media_response(part.data.clone()))
            .unwrap_or_else(res_404),
    }
}

pub async fn segment_ts_handler() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(Body::from("HLS-TS output is not implemented"))
        .expect("valid HLS-TS unsupported response")
}

async fn ensure_store(stream_id: &str, ssrc: u32) -> Option<Arc<HlsStoreHandle>> {
    let existing = cloned_store(&HLS_STORES, ssrc);
    let mut rx = match Register::get_playable_muxer_rx(stream_id, MuxerEnum::HlsMp4) {
        Ok(Some(rx)) => rx,
        Ok(None) => return None,
        Err(_) => return existing,
    };
    let channel_id = rx.channel_id();
    if let Some(store) = existing {
        if store.channel_id >= channel_id {
            return Some(store);
        }
        remove_store_if_same(&HLS_STORES, ssrc, &store);
    }
    let (tx, header_rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::HlsFmp4Header(tx))).ok()?;
    let init = header_rx.await.ok()?;
    let mut state = HlsStore {
        init,
        segments: VecDeque::with_capacity(HLS_WINDOW_SIZE),
        pending: None,
        retired: VecDeque::new(),
        retired_inits: VecDeque::new(),
        epoch: None,
        init_generation: 0,
        discontinuity_sequence: 0,
        next_discontinuity: false,
        ended: false,
    };
    loop {
        match rx.try_recv() {
            Ok(packet) => {
                push_hls_packet(&mut state, packet);
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => reset_media(&mut state),
            Err(broadcast::error::TryRecvError::Empty) => break,
            Err(broadcast::error::TryRecvError::Closed) => {
                state.ended = true;
                break;
            }
        }
    }
    let store = Arc::new(HlsStoreHandle {
        channel_id,
        state: RwLock::new(state),
        notify: Notify::new(),
    });
    let installed = install_store(&HLS_STORES, ssrc, store.clone());
    if !Arc::ptr_eq(&installed, &store) {
        return Some(installed);
    }
    let task_store = store.clone();
    base::tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(packet) => {
                    let changed = {
                        let mut state = task_store.state.write().await;
                        push_hls_packet(&mut state, packet)
                    };
                    if changed {
                        task_store.notify.notify_waiters();
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let mut state = task_store.state.write().await;
                    reset_media(&mut state);
                    drop(state);
                    task_store.notify.notify_waiters();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        task_store.state.write().await.ended = true;
        task_store.notify.notify_waiters();
        base::tokio::time::sleep(Duration::from_secs(30)).await;
        remove_store_if_same(&HLS_STORES, ssrc, &task_store);
    });
    Some(store)
}

fn push_hls_packet(state: &mut HlsStore, packet: Arc<MuxPacket>) -> bool {
    purge_retired(state);
    let Some(meta) = packet.hls.as_ref() else {
        return false;
    };
    if state.epoch.is_some_and(|epoch| epoch != packet.epoch) {
        reset_media(state);
    }
    state.epoch = Some(packet.epoch);
    state.ended = false;
    if let Some(init_segment) = &meta.init_segment {
        state.init = init_segment.clone();
    }

    if state.pending.as_ref().is_some_and(|pending| {
        pending.seq != meta.segment_seq || pending.parts.len() != meta.part_seq
    }) {
        if meta.part_seq != 0 || !can_start_segment(state, meta, packet.is_key) {
            return false;
        }
        reset_media(state);
        state.epoch = Some(packet.epoch);
    }
    if state.pending.is_none() {
        if meta.part_seq != 0 || !can_start_segment(state, meta, packet.is_key) {
            return false;
        }
        state.pending = Some(PendingSegment {
            seq: meta.segment_seq,
            data: Vec::new(),
            duration_us: 0,
            parts: Vec::new(),
            discontinuity: std::mem::take(&mut state.next_discontinuity),
        });
    }

    let pending = state.pending.as_mut().expect("pending HLS segment exists");
    pending.data.extend_from_slice(&packet.data);
    pending.duration_us = pending.duration_us.saturating_add(meta.duration_us);
    pending.parts.push(HlsPart {
        segment_seq: meta.segment_seq,
        part_seq: meta.part_seq,
        data: packet.data.clone(),
        duration_us: meta.duration_us,
        independent: packet.is_key,
    });

    if meta.segment_complete {
        let pending = state.pending.take().expect("completed HLS segment exists");
        let independent = pending.parts.first().is_some_and(|part| part.independent);
        state.segments.push_back(HlsSegment {
            seq: pending.seq,
            data: Bytes::from(pending.data),
            duration_us: pending.duration_us,
            parts: pending.parts,
            independent,
            discontinuity: pending.discontinuity,
        });
        trim_segment_window(state);
    }
    true
}

fn can_start_segment(state: &HlsStore, meta: &HlsPartMeta, is_key: bool) -> bool {
    is_key
        || state
            .segments
            .back()
            .is_some_and(|segment| segment.seq.saturating_add(1) == meta.segment_seq)
}

fn reset_media(state: &mut HlsStore) {
    let expires_at = Instant::now() + HLS_MEDIA_RETENTION;
    state.retired_inits.push_back(RetiredInit {
        generation: state.init_generation,
        data: state.init.clone(),
        expires_at,
    });
    state.init_generation = state.init_generation.saturating_add(1);
    for segment in state.segments.drain(..) {
        if segment.discontinuity {
            state.discontinuity_sequence = state.discontinuity_sequence.saturating_add(1);
        }
        state.retired.push_back(RetiredMedia {
            segment_seq: segment.seq,
            segment: Some(segment.data),
            parts: segment.parts,
            expires_at,
        });
    }
    if let Some(pending) = state.pending.take() {
        if pending.discontinuity {
            state.discontinuity_sequence = state.discontinuity_sequence.saturating_add(1);
        }
        state.retired.push_back(RetiredMedia {
            segment_seq: pending.seq,
            segment: None,
            parts: pending.parts,
            expires_at,
        });
    }
    state.epoch = None;
    state.next_discontinuity = true;
    purge_retired(state);
}

fn trim_segment_window(state: &mut HlsStore) {
    while state.segments.len() > HLS_WINDOW_SIZE {
        let Some(next_independent_index) = state
            .segments
            .iter()
            .skip(1)
            .position(|segment| segment.independent)
        else {
            break;
        };
        let remove_count = next_independent_index + 1;
        let remaining_duration_us = state
            .segments
            .iter()
            .skip(remove_count)
            .map(|segment| segment.duration_us)
            .sum::<u64>();
        if remaining_duration_us < HLS_MIN_PLAYLIST_DURATION_US {
            break;
        }
        for _ in 0..remove_count {
            let segment = state.segments.pop_front().expect("HLS segment exists");
            if segment.discontinuity {
                state.discontinuity_sequence = state.discontinuity_sequence.saturating_add(1);
            }
            state.retired.push_back(RetiredMedia {
                segment_seq: segment.seq,
                segment: Some(segment.data),
                parts: segment.parts,
                expires_at: Instant::now() + HLS_MEDIA_RETENTION,
            });
        }
    }
}

fn purge_retired(state: &mut HlsStore) {
    let now = Instant::now();
    while state
        .retired
        .front()
        .is_some_and(|media| media.expires_at <= now)
    {
        state.retired.pop_front();
    }
    while state
        .retired_inits
        .front()
        .is_some_and(|init| init.expires_at <= now)
    {
        state.retired_inits.pop_front();
    }
}

fn render_playlist(
    state: &HlsStore,
    stream_id: &str,
    encoded_token: &str,
    profile: PlaylistProfile,
) -> String {
    let first_seq = state
        .segments
        .front()
        .map(|segment| segment.seq)
        .or_else(|| state.pending.as_ref().map(|segment| segment.seq))
        .unwrap_or(0);
    let version = if profile == PlaylistProfile::LowLatency {
        10
    } else {
        7
    };
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:{version}\n#EXT-X-TARGETDURATION:{HLS_TARGET_DURATION}\n#EXT-X-MEDIA-SEQUENCE:{first_seq}\n"
    );
    if state.discontinuity_sequence > 0 {
        playlist.push_str(&format!(
            "#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
            state.discontinuity_sequence
        ));
    }
    if profile == PlaylistProfile::LowLatency {
        playlist.push_str(&format!(
            "#EXT-X-PART-INF:PART-TARGET={:.6}\n#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK={HLS_PART_HOLD_BACK_SECONDS:.6}\n",
            seconds(HLS_PART_TARGET_US)
        ));
    }
    playlist.push_str(&format!(
        "#EXT-X-MAP:URI=\"{stream_id}.hmp4?gmv-token={encoded_token}&gmv-hls-generation={}\"\n",
        state.init_generation
    ));
    let part_window_start = state
        .segments
        .len()
        .saturating_sub(HLS_PART_WINDOW_SEGMENTS);
    for (index, segment) in state.segments.iter().enumerate() {
        if segment.discontinuity {
            playlist.push_str("#EXT-X-DISCONTINUITY\n");
        }
        if profile == PlaylistProfile::LowLatency && index >= part_window_start {
            for part in &segment.parts {
                append_part(&mut playlist, stream_id, encoded_token, part);
            }
        }
        playlist.push_str(&format!(
            "#EXTINF:{:.6},\n{stream_id}-{}.m4s?gmv-token={encoded_token}\n",
            seconds(segment.duration_us),
            segment.seq
        ));
    }
    if profile == PlaylistProfile::LowLatency {
        if let Some(pending) = &state.pending {
            if pending.discontinuity {
                playlist.push_str("#EXT-X-DISCONTINUITY\n");
            }
            for part in &pending.parts {
                append_part(&mut playlist, stream_id, encoded_token, part);
            }
        }
    }
    if state.ended {
        playlist.push_str("#EXT-X-ENDLIST\n");
    } else if profile == PlaylistProfile::LowLatency {
        let (segment_seq, part_seq) = next_part(state);
        playlist.push_str(&format!(
            "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"{stream_id}-part-{segment_seq}-{part_seq}.m4s?gmv-token={encoded_token}\"\n"
        ));
    }
    playlist
}

fn find_init(state: &HlsStore, generation: u64) -> Option<&Bytes> {
    if generation == state.init_generation {
        return Some(&state.init);
    }
    let now = Instant::now();
    state
        .retired_inits
        .iter()
        .find(|init| init.generation == generation && init.expires_at > now)
        .map(|init| &init.data)
}

fn append_part(playlist: &mut String, stream_id: &str, encoded_token: &str, part: &HlsPart) {
    let independent = if part.independent {
        ",INDEPENDENT=YES"
    } else {
        ""
    };
    playlist.push_str(&format!(
        "#EXT-X-PART:DURATION={:.6},URI=\"{stream_id}-part-{}-{}.m4s?gmv-token={encoded_token}\"{independent}\n",
        seconds(part.duration_us),
        part.segment_seq,
        part.part_seq
    ));
}

fn seconds(duration_us: u64) -> f64 {
    duration_us as f64 / 1_000_000.0
}

fn next_part(state: &HlsStore) -> (usize, usize) {
    if let Some(pending) = &state.pending {
        return (pending.seq, pending.parts.len());
    }
    (
        state
            .segments
            .back()
            .map(|segment| segment.seq + 1)
            .unwrap_or(0),
        0,
    )
}

fn parse_delivery_directives(query: &HashMap<String, String>) -> Result<DeliveryDirectives, ()> {
    let msn = query
        .get("_HLS_msn")
        .map(|value| value.parse())
        .transpose()
        .map_err(|_| ())?;
    let part = query
        .get("_HLS_part")
        .map(|value| value.parse())
        .transpose()
        .map_err(|_| ())?;
    if part.is_some() && msn.is_none() {
        return Err(());
    }
    Ok(DeliveryDirectives { msn, part })
}

fn delivery_directives_valid(state: &HlsStore, directives: DeliveryDirectives) -> bool {
    let Some(msn) = directives.msn else {
        return true;
    };
    if state.ended {
        return true;
    }
    let directives = normalize_delivery_directives(state, directives);
    let msn = directives.msn.unwrap_or(msn);
    let latest_msn = state
        .pending
        .as_ref()
        .map(|segment| segment.seq)
        .or_else(|| state.segments.back().map(|segment| segment.seq))
        .unwrap_or(0);
    if msn > latest_msn.saturating_add(2) {
        return false;
    }
    let Some(part) = directives.part else {
        return true;
    };
    let advance_part_limit = 3;
    if msn > latest_msn {
        return part <= advance_part_limit;
    }
    let last_part = state
        .pending
        .as_ref()
        .filter(|segment| segment.seq == msn)
        .map(|segment| segment.parts.len().saturating_sub(1))
        .or_else(|| {
            state
                .segments
                .iter()
                .find(|segment| segment.seq == msn)
                .map(|segment| segment.parts.len().saturating_sub(1))
        });
    last_part.is_none_or(|last_part| part <= last_part.saturating_add(advance_part_limit))
}

async fn wait_for_playlist(store: &HlsStoreHandle, directives: DeliveryDirectives) -> bool {
    if directives.msn.is_none() {
        return true;
    }
    let wait = async {
        loop {
            let notified = store.notify.notified();
            let state = store.state.read().await;
            if playlist_available(&state, directives) {
                return;
            }
            drop(state);
            notified.await;
        }
    };
    base::tokio::time::timeout(HLS_BLOCK_RELOAD_TIMEOUT, wait)
        .await
        .is_ok()
}

fn playlist_available(state: &HlsStore, directives: DeliveryDirectives) -> bool {
    let directives = normalize_delivery_directives(state, directives);
    let Some(msn) = directives.msn else {
        return true;
    };
    if state.ended {
        return true;
    }
    if state.segments.iter().any(|segment| segment.seq >= msn) {
        return true;
    }
    let Some(pending) = &state.pending else {
        return false;
    };
    if pending.seq > msn {
        return true;
    }
    if pending.seq < msn {
        return false;
    }
    match directives.part {
        Some(part) => pending.parts.len() > part,
        None => false,
    }
}

fn normalize_delivery_directives(
    state: &HlsStore,
    directives: DeliveryDirectives,
) -> DeliveryDirectives {
    let (Some(msn), Some(part)) = (directives.msn, directives.part) else {
        return directives;
    };
    if state
        .segments
        .iter()
        .find(|segment| segment.seq == msn)
        .is_some_and(|segment| part >= segment.parts.len())
    {
        return DeliveryDirectives {
            msn: Some(msn.saturating_add(1)),
            part: Some(0),
        };
    }
    directives
}

async fn wait_for_part(store: &HlsStoreHandle, segment_seq: usize, part_seq: usize) {
    let wait = async {
        loop {
            let notified = store.notify.notified();
            let state = store.state.read().await;
            if find_part(&state, segment_seq, part_seq).is_some()
                || media_request_expired(&state, segment_seq)
                || state.ended
            {
                return;
            }
            drop(state);
            notified.await;
        }
    };
    let _ = base::tokio::time::timeout(HLS_PART_BLOCK_TIMEOUT, wait).await;
}

fn find_part(state: &HlsStore, segment_seq: usize, part_seq: usize) -> Option<&HlsPart> {
    state
        .segments
        .iter()
        .find(|segment| segment.seq == segment_seq)
        .and_then(|segment| segment.parts.get(part_seq))
        .or_else(|| {
            state
                .pending
                .as_ref()
                .filter(|segment| segment.seq == segment_seq)
                .and_then(|segment| segment.parts.get(part_seq))
        })
        .or_else(|| {
            let now = Instant::now();
            state
                .retired
                .iter()
                .find(|media| media.segment_seq == segment_seq && media.expires_at > now)
                .and_then(|media| media.parts.get(part_seq))
        })
}

fn find_segment(state: &HlsStore, segment_seq: usize) -> Option<&Bytes> {
    state
        .segments
        .iter()
        .find(|segment| segment.seq == segment_seq)
        .map(|segment| &segment.data)
        .or_else(|| {
            let now = Instant::now();
            state
                .retired
                .iter()
                .find(|media| media.segment_seq == segment_seq && media.expires_at > now)
                .and_then(|media| media.segment.as_ref())
        })
}

fn media_request_expired(state: &HlsStore, segment_seq: usize) -> bool {
    state
        .segments
        .front()
        .is_some_and(|segment| segment.seq > segment_seq)
        || state
            .pending
            .as_ref()
            .is_some_and(|segment| segment.seq > segment_seq)
}

fn parse_media_resource(value: &str) -> Option<(&str, MediaResource)> {
    if let Some((stream_id, suffix)) = value.rsplit_once("-part-") {
        let (segment_seq, part_seq) = suffix.split_once('-')?;
        return Some((
            stream_id,
            MediaResource::Part {
                segment_seq: segment_seq.parse().ok()?,
                part_seq: part_seq.parse().ok()?,
            },
        ));
    }
    let (stream_id, seq) = value.rsplit_once('-')?;
    Some((stream_id, MediaResource::Segment(seq.parse().ok()?)))
}

fn cloned_store(
    stores: &DashMap<u32, Arc<HlsStoreHandle>>,
    ssrc: u32,
) -> Option<Arc<HlsStoreHandle>> {
    stores.get(&ssrc).map(|store| store.value().clone())
}

fn install_store(
    stores: &DashMap<u32, Arc<HlsStoreHandle>>,
    ssrc: u32,
    candidate: Arc<HlsStoreHandle>,
) -> Arc<HlsStoreHandle> {
    match stores.entry(ssrc) {
        base::dashmap::mapref::entry::Entry::Occupied(mut entry) => {
            if entry.get().channel_id >= candidate.channel_id {
                return entry.get().clone();
            }
            entry.insert(candidate.clone());
        }
        base::dashmap::mapref::entry::Entry::Vacant(entry) => {
            entry.insert(candidate.clone());
        }
    }
    candidate
}

fn remove_store_if_same(
    stores: &DashMap<u32, Arc<HlsStoreHandle>>,
    ssrc: u32,
    expected: &Arc<HlsStoreHandle>,
) -> bool {
    stores
        .remove_if(&ssrc, |_, current| Arc::ptr_eq(current, expected))
        .is_some()
}

fn media_response(data: Bytes) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_LENGTH, data.len())
        .body(Body::from(data))
        .expect("valid HLS media response")
}

fn status_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("valid HLS status response")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_store() -> HlsStore {
        HlsStore {
            init: Bytes::new(),
            segments: VecDeque::new(),
            pending: None,
            retired: VecDeque::new(),
            retired_inits: VecDeque::new(),
            epoch: None,
            init_generation: 0,
            discontinuity_sequence: 0,
            next_discontinuity: false,
            ended: false,
        }
    }

    fn store(channel_id: u64, ended: bool) -> Arc<HlsStoreHandle> {
        let mut state = empty_store();
        state.ended = ended;
        Arc::new(HlsStoreHandle {
            channel_id,
            state: RwLock::new(state),
            notify: Notify::new(),
        })
    }

    fn packet(
        epoch: Instant,
        segment_seq: usize,
        part_seq: usize,
        duration_us: u64,
        segment_complete: bool,
        is_key: bool,
        data: &'static [u8],
    ) -> Arc<MuxPacket> {
        Arc::new(MuxPacket {
            data: Bytes::from_static(data),
            is_key,
            timestamp: 0,
            epoch,
            seq: segment_seq * 10 + part_seq,
            hls: Some(HlsPartMeta {
                segment_seq,
                part_seq,
                duration_us,
                segment_complete,
                init_segment: (part_seq == 0).then(|| Bytes::from_static(b"init")),
            }),
        })
    }

    #[test]
    fn cloned_hls_store_does_not_hold_map_guard_during_cleanup() {
        let stores = DashMap::new();
        let old = store(1, true);
        stores.insert(1, old.clone());

        let cloned = cloned_store(&stores, 1).unwrap();

        assert!(Arc::ptr_eq(&cloned, &old));
        assert!(remove_store_if_same(&stores, 1, &cloned));
        assert!(!stores.contains_key(&1));
    }

    #[test]
    fn stale_hls_cleanup_does_not_remove_a_new_store_generation() {
        let stores = DashMap::new();
        let old = store(1, true);
        let current = store(2, false);
        stores.insert(1, old.clone());
        assert!(remove_store_if_same(&stores, 1, &old));

        stores.insert(1, current.clone());
        assert!(!remove_store_if_same(&stores, 1, &old));
        assert!(Arc::ptr_eq(stores.get(&1).unwrap().value(), &current));
    }

    #[test]
    fn stale_hls_request_does_not_replace_a_new_store_generation() {
        let stores = DashMap::new();
        let current = store(2, false);
        let stale = store(1, false);
        stores.insert(1, current.clone());

        let installed = install_store(&stores, 1, stale);

        assert!(Arc::ptr_eq(&installed, &current));
        assert!(Arc::ptr_eq(stores.get(&1).unwrap().value(), &current));
    }

    #[test]
    fn hls_parts_form_a_parent_segment_with_exact_duration() {
        let epoch = Instant::now();
        let mut state = empty_store();

        assert!(push_hls_packet(
            &mut state,
            packet(epoch, 7, 0, 450_000, false, true, b"part-0")
        ));
        assert!(push_hls_packet(
            &mut state,
            packet(epoch, 7, 1, 480_000, true, false, b"part-1")
        ));

        let segment = state.segments.front().unwrap();
        assert_eq!(segment.seq, 7);
        assert_eq!(segment.duration_us, 930_000);
        assert_eq!(&segment.data[..], b"part-0part-1");
        assert_eq!(segment.parts.len(), 2);
        assert!(state.pending.is_none());
    }

    #[test]
    fn ll_hls_playlist_advertises_parts_blocking_reload_and_preload_hint() {
        let epoch = Instant::now();
        let mut state = empty_store();
        push_hls_packet(
            &mut state,
            packet(epoch, 3, 0, 450_000, false, true, b"part-0"),
        );

        let playlist = render_playlist(&state, "live", "token", PlaylistProfile::LowLatency);

        assert!(playlist.contains("#EXT-X-VERSION:10"));
        assert!(playlist.contains("#EXT-X-PART-INF:PART-TARGET=0.500000"));
        assert!(playlist.contains("CAN-BLOCK-RELOAD=YES"));
        assert!(playlist.contains("#EXT-X-PART:DURATION=0.450000"));
        assert!(playlist.contains("INDEPENDENT=YES"));
        assert!(playlist.contains("live-part-3-1.m4s"));
        assert!(!playlist.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn standard_hls_playlist_only_advertises_complete_segments() {
        let epoch = Instant::now();
        let mut state = empty_store();
        push_hls_packet(
            &mut state,
            packet(epoch, 3, 0, 450_000, false, true, b"pending"),
        );
        push_hls_packet(
            &mut state,
            packet(epoch, 3, 1, 480_000, true, false, b"complete"),
        );
        push_hls_packet(
            &mut state,
            packet(epoch, 4, 0, 450_000, false, true, b"next"),
        );

        let playlist = render_playlist(&state, "live", "token", PlaylistProfile::Standard);

        assert!(playlist.contains("#EXT-X-VERSION:7"));
        assert!(playlist.contains("#EXTINF:0.930000"));
        assert!(playlist.contains("live-3.m4s"));
        assert!(!playlist.contains("#EXT-X-PART"));
        assert!(!playlist.contains("#EXT-X-SERVER-CONTROL"));
        assert!(!playlist.contains("#EXT-X-PRELOAD-HINT"));
        assert!(!playlist.contains("live-4.m4s"));
    }

    #[test]
    fn ended_ll_hls_playlist_has_endlist_without_preload_hint() {
        let mut state = empty_store();
        state.ended = true;

        let playlist = render_playlist(&state, "live", "token", PlaylistProfile::LowLatency);

        assert!(playlist.contains("#EXT-X-ENDLIST"));
        assert!(!playlist.contains("#EXT-X-PRELOAD-HINT"));
    }

    #[test]
    fn playlist_delivery_directive_waits_for_requested_part() {
        let epoch = Instant::now();
        let mut state = empty_store();
        push_hls_packet(
            &mut state,
            packet(epoch, 4, 0, 900_000, false, true, b"part-0"),
        );

        assert!(playlist_available(
            &state,
            DeliveryDirectives {
                msn: Some(4),
                part: Some(0)
            }
        ));
        assert!(!playlist_available(
            &state,
            DeliveryDirectives {
                msn: Some(4),
                part: Some(1)
            }
        ));
    }

    #[test]
    fn segment_only_delivery_directive_waits_for_complete_segment() {
        let epoch = Instant::now();
        let mut state = empty_store();
        push_hls_packet(
            &mut state,
            packet(epoch, 4, 0, 900_000, false, true, b"part-0"),
        );

        assert!(!playlist_available(
            &state,
            DeliveryDirectives {
                msn: Some(4),
                part: None
            }
        ));
        push_hls_packet(
            &mut state,
            packet(epoch, 4, 1, 900_000, true, false, b"part-1"),
        );
        assert!(playlist_available(
            &state,
            DeliveryDirectives {
                msn: Some(4),
                part: None
            }
        ));
    }

    #[test]
    fn part_after_completed_parent_waits_for_next_parent_part_zero() {
        let epoch = Instant::now();
        let mut state = empty_store();
        push_hls_packet(
            &mut state,
            packet(epoch, 4, 0, 900_000, true, true, b"segment-4"),
        );
        let directives = DeliveryDirectives {
            msn: Some(4),
            part: Some(1),
        };

        assert!(!playlist_available(&state, directives));
        push_hls_packet(
            &mut state,
            packet(epoch, 5, 0, 900_000, false, true, b"segment-5-part-0"),
        );
        assert!(playlist_available(&state, directives));
    }

    #[test]
    fn invalid_and_excessively_future_delivery_directives_are_rejected() {
        let mut query = HashMap::from([("_HLS_part".to_string(), "0".to_string())]);
        assert!(parse_delivery_directives(&query).is_err());
        query.insert("_HLS_msn".to_string(), "invalid".to_string());
        assert!(parse_delivery_directives(&query).is_err());

        let state = empty_store();
        assert!(!delivery_directives_valid(
            &state,
            DeliveryDirectives {
                msn: Some(3),
                part: Some(0)
            }
        ));
    }

    #[test]
    fn epoch_change_discards_old_playlist_generation() {
        let mut state = empty_store();
        let old_epoch = Instant::now();
        push_hls_packet(
            &mut state,
            packet(old_epoch, 1, 0, 900_000, true, true, b"old"),
        );
        let new_epoch = old_epoch + Duration::from_secs(1);

        push_hls_packet(
            &mut state,
            packet(new_epoch, 2, 0, 900_000, false, true, b"new"),
        );

        assert!(state.segments.is_empty());
        assert_eq!(state.pending.as_ref().map(|segment| segment.seq), Some(2));
        assert_eq!(state.init_generation, 1);
        assert_eq!(
            find_init(&state, 0).map(|init| &init[..]),
            Some(&b"init"[..])
        );
        let playlist = render_playlist(&state, "live", "token", PlaylistProfile::LowLatency);
        assert!(playlist.contains("gmv-hls-generation=1"));
        assert!(playlist.contains("#EXT-X-DISCONTINUITY"));
    }

    #[test]
    fn sliding_hls_window_keeps_six_complete_segments() {
        let epoch = Instant::now();
        let mut state = empty_store();
        for seq in 1..=8 {
            push_hls_packet(
                &mut state,
                packet(epoch, seq, 0, 2_000_000, true, true, b"segment"),
            );
        }

        assert_eq!(state.segments.len(), HLS_WINDOW_SIZE);
        assert_eq!(state.segments.front().map(|segment| segment.seq), Some(3));
        assert_eq!(state.segments.back().map(|segment| segment.seq), Some(8));
        assert_eq!(
            state
                .segments
                .iter()
                .map(|segment| segment.duration_us)
                .sum::<u64>(),
            12_000_000
        );
        assert_eq!(
            find_segment(&state, 1).map(|data| &data[..]),
            Some(&b"segment"[..])
        );
        assert!(find_part(&state, 1, 0).is_some());
    }

    #[test]
    fn sliding_window_waits_for_an_independent_front_and_minimum_duration() {
        let epoch = Instant::now();
        let mut state = empty_store();
        for seq in 1..=7 {
            push_hls_packet(
                &mut state,
                packet(epoch, seq, 0, 2_000_000, true, seq != 2, b"segment"),
            );
        }
        assert_eq!(state.segments.len(), 7);
        assert!(
            state
                .segments
                .front()
                .is_some_and(|segment| segment.independent)
        );

        push_hls_packet(
            &mut state,
            packet(epoch, 8, 0, 2_000_000, true, true, b"segment"),
        );
        assert_eq!(state.segments.len(), HLS_WINDOW_SIZE);
        assert_eq!(state.segments.front().map(|segment| segment.seq), Some(3));
        assert!(
            state
                .segments
                .front()
                .is_some_and(|segment| segment.independent)
        );
    }

    #[test]
    fn media_resource_parser_accepts_segments_and_parts() {
        assert!(matches!(
            parse_media_resource("camera-a-9"),
            Some(("camera-a", MediaResource::Segment(9)))
        ));
        assert!(matches!(
            parse_media_resource("camera-a-part-9-2"),
            Some((
                "camera-a",
                MediaResource::Part {
                    segment_seq: 9,
                    part_seq: 2
                }
            ))
        ));
    }
}
