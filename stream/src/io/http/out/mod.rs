use crate::io::http::{res_401, res_404};
use crate::state::event::{Event, EventRes, OutEvent, OutEventRes};
use crate::state::register::Register;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use base::bytes::Bytes;
use base::log::{debug, info, warn};
use base::tokio::sync::oneshot;
use futures_core::Stream;
use gmv_domain::info::obj::{BaseStreamInfo, PLAY_PATH, StreamPlayInfo};
use gmv_domain::info::output::OutputEnum;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::process::id;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};

mod dash;
mod flv;
mod hls;
//收到流-》media 长期阻塞 ——》无输出流
pub fn routes() -> Router {
    Router::new().route(PLAY_PATH, axum::routing::get(handler))
}

/// 根据HTTP-URL请求播放
async fn handler(
    Path(stream_id): Path<String>,
    Query(mut map): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response<Body> {
    debug!("stream play: stream_id={stream_id}");
    let token: Arc<str> = match map.remove("gmv-token") {
        None => {
            return res_401();
        }
        Some(token) => Arc::from(token),
    };
    match stream_id.rsplit_once('.') {
        None => res_404(),
        Some((id, tp)) => {
            let id = Arc::from(id);
            match tp {
                "flv" => {
                    info!("flv stream play:stream_id: {}, param: {:?}", stream_id, map);
                    flv::handler(id, token, addr).await
                }
                "m3u8" => {
                    let (id, profile) = match id.strip_suffix(".ll") {
                        Some(id) => (Arc::from(id), hls::PlaylistProfile::LowLatency),
                        None => (id, hls::PlaylistProfile::Standard),
                    };
                    hls::m3u8_handler(id, token, addr, &map, profile).await
                }
                "hmp4" => hls::init_mp4_handler(id, token, &map).await,
                "m4s"
                    if id.rsplit_once("-part-").is_some_and(|(_, suffix)| {
                        suffix.split_once('-').is_some_and(|(segment, part)| {
                            segment.parse::<u64>().is_ok() && part.parse::<u64>().is_ok()
                        })
                    }) || id
                        .rsplit_once('-')
                        .is_some_and(|(_, sequence)| sequence.parse::<u64>().is_ok()) =>
                {
                    hls::segment_mp4_handler(id, token).await
                }
                "mp4" => {
                    crate::io::local::mp4::serve_completed(
                        &id,
                        &token,
                        headers
                            .get(header::RANGE)
                            .and_then(|value| value.to_str().ok()),
                    )
                    .await
                }
                "ts" => hls::segment_ts_handler().await,
                "mpd" => {
                    debug!(
                        "mpeg dash mpd stream play:stream_id: {}, param: {:?}",
                        stream_id, map
                    );
                    dash::mpd_handler(id, token, addr).await
                } // MPD manifest
                "m4it" => dash::init_segment(id, token, addr).await, // CMAF init
                "fmp4" => {
                    info!(
                        "fmp4 dash chunk stream play:stream_id: {}, param: {:?}",
                        stream_id, map
                    );
                    dash::chunk(id, token, addr).await // media chunk stream
                }
                "m4s" => {
                    debug!(
                        "mpeg dash segment stream play:stream_id: {}, param: {:?}",
                        stream_id, map
                    );
                    dash::segment(id, token, addr).await
                }
                _ => res_404(),
            }
        }
    }
}

struct DisconnectAwareStream<S> {
    inner: Pin<Box<S>>,
    on_drop: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl<S> Stream for DisconnectAwareStream<S>
where
    S: Stream<Item = Result<Bytes, std::convert::Infallible>>,
{
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<S> Drop for DisconnectAwareStream<S> {
    fn drop(&mut self) {
        if let Some(cb) = self.on_drop.take() {
            cb();
        }
    }
}
pub enum OutPlayKind {
    Play,
    Forbid,
    Notfound,
}

pub async fn stream_user_token_check(
    out: OutputEnum,
    bsi: BaseStreamInfo,
    stream_id: Arc<str>,
    token: Arc<str>,
    addr: SocketAddr,
) -> OutPlayKind {
    match stream_user_token_authorize(out, bsi, stream_id.clone(), token.clone(), addr).await {
        OutPlayKind::Play => match Register::insert_out_token(stream_id, out, token) {
            Ok(_) => OutPlayKind::Play,
            Err(_) => OutPlayKind::Notfound,
        },
        other => other,
    }
}

pub async fn stream_user_token_authorize(
    out: OutputEnum,
    bsi: BaseStreamInfo,
    stream_id: Arc<str>,
    token: Arc<str>,
    addr: SocketAddr,
) -> OutPlayKind {
    if Register::check_token(&(token.clone(), stream_id.clone())) {
        OutPlayKind::Play
    } else {
        let play_info = StreamPlayInfo::new(bsi, Some(addr.to_string()), token.to_string(), out);
        let (tx, rx) = oneshot::channel();
        let event_tx = Register::get_event_tx();
        let _ = event_tx
            .send((Event::Out(OutEvent::OnPlay(play_info)), Some(tx)))
            .await;
        match rx.await {
            Ok(EventRes::Out(OutEventRes::OnPlay(Some(true)))) => OutPlayKind::Play,
            Ok(_) => OutPlayKind::Forbid,
            Err(_) => OutPlayKind::Notfound,
        }
    }
}
