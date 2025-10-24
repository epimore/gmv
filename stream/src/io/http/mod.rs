use axum::Router;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info};
use base::tokio::net::TcpListener;
use base::tokio::select;
use base::tokio::sync::mpsc::Sender;
use base::tokio_util::sync::CancellationToken;
use std::net::SocketAddr;

mod api;
pub mod call;
mod out;

pub fn listen_http_server(port: u16) -> GlobalResult<std::net::TcpListener> {
    let listener =
        std::net::TcpListener::bind(format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    Ok(listener)
}

pub async fn run(
    node: String,
    std_http_listener: std::net::TcpListener,
    tx: Sender<u32>,
    cancel_token: CancellationToken,
) -> GlobalResult<()> {
    std_http_listener
        .set_nonblocking(true)
        .hand_log(|msg| error!("{msg}"))?;
    let listener = TcpListener::from_std(std_http_listener).hand_log(|msg| error!("{msg}"))?;
    let app = Router::new()
        .merge(out::routes(&node))
        .merge(api::routes(tx.clone()));
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        cancel_token.cancelled().await;
    });
    match server.await.hand_log(|msg| error!("{msg}")) {
        Ok(()) => Ok(()),
        error => error,
    }
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
