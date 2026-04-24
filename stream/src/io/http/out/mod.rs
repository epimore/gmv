use crate::io::http::{res_401, res_404};
use crate::state::event::{Event, EventRes, OutEvent, OutEventRes};
use crate::state::register::Register;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query};
use axum::response::Response;
use base::bytes::Bytes;
use base::log::{debug, info, warn};
use base::tokio::sync::oneshot;
use futures_core::Stream;
use shared::info::obj::{BaseStreamInfo, PLAY_PATH, StreamPlayInfo};
use shared::info::output::OutputEnum;
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

#[cfg_attr(
    debug_assertions,
    utoipa::path(
        get,
        path = "/play/{stream_id}",
        request_body = (),
        params(
            ("stream_id" = String, Path, description = "流 ID"),
            ("gmv-token" = String, Query, description = "认证 token", example = "tkn_xyz789")
        ),
        responses(
            (status = 200, description = "成功播放流", body = Vec<u8>, content_type = "video/flv"),
            (status = 401, description = "gmv-token 无效"),
            (status = 404, description = "流未找到"),
            (status = 500, description = "内部服务器错误")
        ),
        tag = "HTTP播放音视频"
    )
)]
/// 根据HTTP-URL请求播放
async fn handler(
    Path(stream_id): Path<String>,
    Query(mut map): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response<Body> {
    debug!("stream play:stream_id: {}, param: {:?}", stream_id, map);
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
                "m3u8" => hls::m3u8_handler().await,
                "hmp4" => hls::segment_mp4_handler().await,
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
    inner: S,
    on_drop: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl<S> Stream for DisconnectAwareStream<S>
where
    S: Stream<Item = Result<Bytes, std::convert::Infallible>> + Unpin,
{
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
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
    if Register::check_token(&(token.clone(), stream_id.clone())) {
        match Register::insert_out_token(stream_id, out, token) {
            Ok(_) => {
                OutPlayKind::Play
            }
            Err(_) => {
                OutPlayKind::Notfound
            }
        }
    } else {
        let play_info = StreamPlayInfo::new(
            bsi,
            Some(addr.to_string()),
            token.to_string(),
            out,
        );
        let (tx, rx) = oneshot::channel();
        let event_tx = Register::get_event_tx();
        let _ = event_tx
            .send((Event::Out(OutEvent::OnPlay(play_info)), Some(tx)))
            .await;
        match rx.await {
            Ok(EventRes::Out(OutEventRes::OnPlay(Some(true)))) => {
                match Register::insert_out_token(stream_id, out, token) {
                    Ok(_) => {
                        OutPlayKind::Play
                    }
                    Err(_) => {
                        OutPlayKind::Notfound
                    }
                }
            }
            Ok(_) => OutPlayKind::Forbid,
            Err(_) => OutPlayKind::Notfound,
        }
    }
}
