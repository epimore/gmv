use crate::io::http::out::{OutPlayKind, stream_user_token_check};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::register::Register;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::once_cell::sync::Lazy;
use base::tokio::sync::{RwLock, broadcast, oneshot};
use gmv_domain::info::output::OutputEnum;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const HLS_WINDOW_SIZE: usize = 6;
const HLS_TARGET_DURATION: u64 = 2;

static HLS_STORES: Lazy<DashMap<u32, Arc<RwLock<HlsStore>>>> = Lazy::new(DashMap::new);

#[derive(Clone)]
struct HlsSegment {
    seq: usize,
    data: Bytes,
}

struct HlsStore {
    init: Bytes,
    segments: VecDeque<HlsSegment>,
    ended: bool,
}

pub async fn m3u8_handler(
    stream_id: Arc<str>,
    token: Arc<str>,
    addr: SocketAddr,
) -> Response<Body> {
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id.clone()) else {
        return res_404();
    };
    let ssrc = base.rtp_info.ssrc;
    match stream_user_token_check(
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
    Register::listen_output_timeout(
        stream_id.clone(),
        OutputEnum::HlsFmp4,
        token.clone(),
        addr,
        0,
    );
    let store = match ensure_store(ssrc).await {
        Some(store) => store,
        None => return res_404(),
    };
    let state = store.read().await;
    let first_seq = state
        .segments
        .front()
        .map(|segment| segment.seq)
        .unwrap_or(0);
    let encoded_token: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{HLS_TARGET_DURATION}\n#EXT-X-MEDIA-SEQUENCE:{first_seq}\n#EXT-X-MAP:URI=\"{stream_id}.hmp4?gmv-token={encoded_token}\"\n"
    );
    for segment in &state.segments {
        playlist.push_str(&format!(
            "#EXTINF:{HLS_TARGET_DURATION}.000,\n{stream_id}-{}.m4s?gmv-token={encoded_token}\n",
            segment.seq
        ));
    }
    if state.ended {
        playlist.push_str("#EXT-X-ENDLIST\n");
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-cache, no-store")
        .body(Body::from(playlist))
        .expect("valid HLS playlist response")
}

pub async fn init_mp4_handler(stream_id: Arc<str>, token: Arc<str>) -> Response<Body> {
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id) else {
        return res_404();
    };
    if !Register::check_token(&(token, Arc::from(base.stream_id.as_str()))) {
        return res_401();
    }
    let Some(store) = ensure_store(base.rtp_info.ssrc).await else {
        return res_404();
    };
    let init = store.read().await.init.clone();
    media_response(init)
}

pub async fn segment_mp4_handler(segment_id: Arc<str>, token: Arc<str>) -> Response<Body> {
    let Some((stream_id, seq)) = segment_id.rsplit_once('-') else {
        return res_404();
    };
    let Ok(seq) = seq.parse::<usize>() else {
        return res_404();
    };
    let stream_id: Arc<str> = Arc::from(stream_id);
    let Some(base) = Register::get_base_stream_info_by_stream_id(stream_id.clone()) else {
        return res_404();
    };
    if !Register::check_token(&(token, stream_id)) {
        return res_401();
    }
    let Some(store) = ensure_store(base.rtp_info.ssrc).await else {
        return res_404();
    };
    let state = store.read().await;
    match state.segments.iter().find(|segment| segment.seq == seq) {
        Some(segment) => media_response(segment.data.clone()),
        None => res_404(),
    }
}

pub async fn segment_ts_handler() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(Body::from("HLS-TS output is not implemented"))
        .expect("valid HLS-TS unsupported response")
}

async fn ensure_store(ssrc: u32) -> Option<Arc<RwLock<HlsStore>>> {
    if let Some(store) = HLS_STORES.get(&ssrc) {
        return Some(store.clone());
    }
    let mut rx = Register::get_muxer_rx(&ssrc, MuxerEnum::HlsMp4).ok()?;
    let (tx, header_rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::HlsFmp4Header(tx))).ok()?;
    let init = header_rx.await.ok()?;
    let store = Arc::new(RwLock::new(HlsStore {
        init,
        segments: VecDeque::with_capacity(HLS_WINDOW_SIZE),
        ended: false,
    }));
    match HLS_STORES.entry(ssrc) {
        base::dashmap::mapref::entry::Entry::Occupied(entry) => return Some(entry.get().clone()),
        base::dashmap::mapref::entry::Entry::Vacant(entry) => {
            entry.insert(store.clone());
        }
    }
    let task_store = store.clone();
    base::tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(packet) => {
                    let mut state = task_store.write().await;
                    if state.segments.is_empty() && !packet.is_key {
                        continue;
                    }
                    state.segments.push_back(HlsSegment {
                        seq: packet.seq,
                        data: packet.data.clone(),
                    });
                    while state.segments.len() > HLS_WINDOW_SIZE {
                        state.segments.pop_front();
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    task_store.write().await.segments.clear();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        task_store.write().await.ended = true;
        base::tokio::time::sleep(Duration::from_secs(30)).await;
        HLS_STORES.remove(&ssrc);
    });
    Some(store)
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
