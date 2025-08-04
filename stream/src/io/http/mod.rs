use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Router;
use common::exception::{GlobalResult, GlobalResultExt};
use common::log::{error, info};
use common::tokio::net::TcpListener;
use common::tokio::sync::mpsc::Sender;
use std::net::SocketAddr;

mod flv;
mod hls;
mod dash;
mod api;
pub mod call;

pub fn listen_http_server(port: u16) -> GlobalResult<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    info!("Listen to http web addr = 0.0.0.0:{} ...", port);
    Ok(listener)
}

pub async fn run(node: &String, std_http_listener: std::net::TcpListener, tx: Sender<u32>) -> GlobalResult<()> {
    std_http_listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
    let listener = TcpListener::from_std(std_http_listener).hand_log(|msg| error!("{msg}"))?;
    let app = Router::new()
        .merge(flv::routes(node))
        .merge(hls::routes())
        // .merge(dash::routes())
        .merge(api::routes(tx.clone()));

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

/// 404 Not Found
pub fn res_404() -> Response<Body> {
    Response::builder()
        .header("Content-Type", "text/plain")
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("404 Not Found"))
        .unwrap()
}

/// 401 Unauthorized
pub fn res_401() -> Response<Body> {
    Response::builder()
        .header("Content-Type", "text/plain")
        .status(StatusCode::UNAUTHORIZED)
        .body(Body::from("401 Unauthorized"))
        .unwrap()
}

/// 500 Internal Server Error
pub fn res_500() -> Response<Body> {
    Response::builder()
        .header("Content-Type", "text/plain")
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("500 Internal Server Error"))
        .unwrap()
}

use common::bytes::Bytes;
use futures_core::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

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
