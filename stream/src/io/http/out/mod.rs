use std::net::SocketAddr;
use axum::Router;
use shared::info::obj::{PLAY_PATH};
use base::bytes::Bytes;
use futures_core::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query};
use axum::response::Response;
use crate::io::http::{res_401, res_404};

mod flv;
mod hls;
mod dash;

pub fn routes() -> Router {
    Router::new().route(PLAY_PATH, axum::routing::get(handler))
}
async fn handler(Path(stream_id): Path<String>, Query(token): Query<Option<String>>, ConnectInfo(addr): ConnectInfo<SocketAddr>)
                 -> Response<Body> {
    if token.is_none() {
        return res_401();
    }
    match stream_id.rsplit_once(',') {
        None => { res_404() }
        Some((id, tp)) => {
            match tp {
                "flv" => { flv::handler(id.to_string(), token.unwrap(), addr).await }
                "m3u8" => { hls::m3u8_handler().await }
                "ts" => { hls::segment_handler().await }
                "mp4" => { dash::mpd_handler().await }
                "m4s" => { dash::segment_handler().await }
                _ => { res_404() }
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

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
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