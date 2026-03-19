use crate::io::http::{res_401, res_404};
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query};
use axum::response::Response;
use base::bytes::Bytes;
use base::log::{debug, info, warn};
use futures_core::Stream;
use shared::info::obj::PLAY_PATH;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::str::FromStr;
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
    Query(map): Query<HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response<Body> {
    debug!("stream play:stream_id: {}, param: {:?}", stream_id, map);
    let token = map.get("gmv-token");
    if token.is_none() {
        return res_401();
    }
    let token = token.unwrap().clone();
    match stream_id.rsplit_once('.') {
        None => res_404(),
        Some((id, tp)) => {
            match tp {
                "flv" => {
                    info!("flv stream play:stream_id: {}, param: {:?}", stream_id, map);
                    flv::handler(id.to_string(), &token, addr).await
                }
                "m3u8" => hls::m3u8_handler().await,
                "ts" => hls::segment_handler().await,
                "mpd" => {
                    debug!("mpeg dash mpd stream play:stream_id: {}, param: {:?}", stream_id, map);
                    dash::mpd_handler(id.to_string(), &token, addr).await
                }, // MPD manifest
                "m4it" => dash::init_segment(id.to_string(), &token).await, // CMAF init
                "fmp4" => {
                    info!("fmp4 dash chunk stream play:stream_id: {}, param: {:?}", stream_id, map);
                    dash::chunk(id.to_string(), &token, addr).await // media chunk stream
                }
                "m4s" => {
                    debug!("mpeg dash segment stream play:stream_id: {}, param: {:?}", stream_id, map);
                    dash::segment(id.to_string(), &token).await
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
    S: Stream<Item=Result<Bytes, std::convert::Infallible>> + Unpin,
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
