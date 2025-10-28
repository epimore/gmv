use std::net::SocketAddr;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Router;
use base::cfg_lib::conf;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, info};
use base::serde::Deserialize;
use base::serde_default;
use base::tokio::net::TcpListener;
use base::tokio_util::sync::CancellationToken;

mod api;
mod hook;
mod edge;
pub mod client;

pub const UPLOAD_PICTURE: &str = "/edge/upload/picture";
#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "http")]
pub struct Http {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_timeout")]
    pub timeout: u16,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default = "default_version")]
    pub version: String,
}
serde_default!(default_port, u16, 8080);
serde_default!(default_timeout, u16, 30);
serde_default!(default_prefix, String, "/gmv".to_string());
serde_default!(default_server_name, String, "web-server".to_string());
serde_default!(default_version, String, "v0.1".to_string());
impl Http {
    pub fn get_http_by_conf() -> Self {
        Http::conf()
    }

    pub fn listen_http_server(&self) -> GlobalResult<std::net::TcpListener> {
        let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", self.port)).hand_log(|msg| error!("{msg}"))?;
        Ok(listener)
    }

    pub async fn run(&self, listener: std::net::TcpListener, cancel_token: CancellationToken) -> GlobalResult<()> {
        listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
        let listener = TcpListener::from_std(listener).hand_log(|msg| error!("{msg}"))?;
        // 创建包含所有路由的统一Router
        let all_routes = Router::new()
            .merge(edge::routes())
            .nest("/hook",hook::routes())
            .merge(api::routes());

        // 为所有路由添加`/v1`前缀
        let app = Router::new()
            .nest(&format!("{}",self.prefix), all_routes);

        let server = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
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
}