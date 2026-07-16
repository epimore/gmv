use crate::io::http::out::{OutPlayKind, stream_user_token_check};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
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

static HLS_STORES: Lazy<DashMap<u32, Arc<HlsStoreHandle>>> = Lazy::new(DashMap::new);

#[derive(Clone)]
struct HlsSegment {
    seq: usize,
    data: Bytes,
    is_key: bool,
}

struct HlsStore {
    init: Bytes,
    segments: VecDeque<HlsSegment>,
    ended: bool,
}

struct HlsStoreHandle {
    channel_id: u64,
    state: RwLock<HlsStore>,
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
    let state = store.state.read().await;
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
    let init = store.state.read().await.init.clone();
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
    let state = store.state.read().await;
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

async fn ensure_store(ssrc: u32) -> Option<Arc<HlsStoreHandle>> {
    let mut rx = Register::get_muxer_rx(&ssrc, MuxerEnum::HlsMp4).ok()?;
    let channel_id = rx.channel_id();
    if let Some(store) = cloned_store(&HLS_STORES, ssrc) {
        // A request from the previous output generation may finish after the replacement store.
        if store.channel_id > channel_id {
            return Some(store);
        }
        if store.channel_id == channel_id && !store.state.read().await.ended {
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
        ended: false,
    };
    loop {
        match rx.try_recv() {
            Ok(packet) => push_hls_packet(&mut state, packet),
            Err(broadcast::error::TryRecvError::Lagged(_)) => state.segments.clear(),
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
                    let mut state = task_store.state.write().await;
                    push_hls_packet(&mut state, packet);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    task_store.state.write().await.segments.clear();
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        task_store.state.write().await.ended = true;
        base::tokio::time::sleep(Duration::from_secs(30)).await;
        remove_store_if_same(&HLS_STORES, ssrc, &task_store);
    });
    Some(store)
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

fn push_hls_packet(state: &mut HlsStore, packet: Arc<MuxPacket>) {
    if state.segments.is_empty() && !packet.is_key {
        return;
    }
    state.segments.push_back(HlsSegment {
        seq: packet.seq,
        data: packet.data.clone(),
        is_key: packet.is_key,
    });
    trim_segment_window(&mut state.segments);
}

fn trim_segment_window(segments: &mut VecDeque<HlsSegment>) {
    while segments.len() > HLS_WINDOW_SIZE {
        let Some(next_key_index) = segments.iter().skip(1).position(|segment| segment.is_key)
        else {
            break;
        };
        for _ in 0..=next_key_index {
            segments.pop_front();
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn store(channel_id: u64, ended: bool) -> Arc<HlsStoreHandle> {
        Arc::new(HlsStoreHandle {
            channel_id,
            state: RwLock::new(HlsStore {
                init: Bytes::new(),
                segments: VecDeque::new(),
                ended,
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
    fn sliding_hls_window_keeps_a_key_fragment_at_its_front() {
        let mut segments = VecDeque::new();
        for seq in 1..=8 {
            segments.push_back(HlsSegment {
                seq,
                data: Bytes::new(),
                is_key: seq == 1 || seq == 5,
            });
            trim_segment_window(&mut segments);
        }

        assert_eq!(segments.front().map(|segment| segment.seq), Some(5));
        assert!(segments.front().is_some_and(|segment| segment.is_key));
    }

    #[test]
    fn hls_store_starts_from_a_replayed_key_fragment() {
        let epoch = Instant::now();
        let mut state = HlsStore {
            init: Bytes::new(),
            segments: VecDeque::new(),
            ended: false,
        };
        push_hls_packet(
            &mut state,
            Arc::new(MuxPacket {
                data: Bytes::from_static(b"delta"),
                is_key: false,
                timestamp: 1,
                epoch,
                seq: 1,
            }),
        );
        assert!(state.segments.is_empty());

        push_hls_packet(
            &mut state,
            Arc::new(MuxPacket {
                data: Bytes::from_static(b"key"),
                is_key: true,
                timestamp: 2,
                epoch,
                seq: 2,
            }),
        );
        assert_eq!(state.segments.front().map(|segment| segment.seq), Some(2));
        assert!(state.segments.front().is_some_and(|segment| segment.is_key));
    }
}
