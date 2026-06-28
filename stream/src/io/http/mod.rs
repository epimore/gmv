use axum::Router;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::mpsc::Sender;
use base::tokio_util::sync::CancellationToken;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

mod api;
pub mod call;
mod out;

#[derive(Debug, Clone)]
pub struct HttpTlsConfig {
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

pub fn listen_http_server(port: u16) -> GlobalResult<std::net::TcpListener> {
    let listener =
        std::net::TcpListener::bind(format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    Ok(listener)
}

pub async fn run(
    std_http_listener: std::net::TcpListener,
    tls: Option<HttpTlsConfig>,
    _tx: Sender<u32>,
    cancel_token: CancellationToken,
) -> GlobalResult<()> {
    std_http_listener
        .set_nonblocking(true)
        .hand_log(|msg| error!("{msg}"))?;
    let app = Router::new().merge(out::routes()).merge(api::routes());
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    let handle = axum_server::Handle::new();
    let shutdown = handle.clone();
    base::tokio::spawn(async move {
        cancel_token.cancelled().await;
        shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
    });
    let result = if let Some(tls) = tls {
        let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            tls.certificate_path,
            tls.private_key_path,
        )
        .await
        .hand_log(|msg| error!("{msg}"))?;
        axum_server::from_tcp_rustls(std_http_listener, rustls)
            .hand_log(|msg| error!("{msg}"))?
            .handle(handle)
            .serve(service)
            .await
    } else {
        axum_server::from_tcp(std_http_listener)
            .hand_log(|msg| error!("{msg}"))?
            .handle(handle)
            .serve(service)
            .await
    };
    result.hand_log(|msg| error!("{msg}"))
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
