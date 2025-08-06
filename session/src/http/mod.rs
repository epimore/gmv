use std::net::SocketAddr;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Router;
use common::cfg_lib::conf;
use common::exception::{GlobalResult, GlobalResultExt};
use common::log::{error, info};
use common::serde::Deserialize;
use common::serde_default;
use common::tokio::net::TcpListener;
use common::tokio::sync::mpsc::Sender;
use crate::general::http::Http;

mod api;
mod call;
mod edge;

pub const UPLOAD_PICTURE: &str = "/edge/upload/picture";
pub const PUSH_PICTURE: &str = "/edge/push/picture";
pub const PUSH_PICTURE: &str = "/edge/push/picture";
pub const PUSH_PICTURE: &str = "/edge/push/picture";
pub const PUSH_PICTURE: &str = "/edge/push/picture";
#[derive(Debug, Deserialize)]
#[serde(crate = "common::serde")]
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
        info!("Listen to http web addr = 0.0.0.0:{} ...", port);
        Ok(listener)
    }

    pub async fn run(&self, listener: std::net::TcpListener) -> GlobalResult<()> {
        listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
        let listener = TcpListener::from_std(listener).hand_log(|msg| error!("{msg}"))?;
        let app = Router::new()
            .merge(edge::routes())
            .merge(api::routes());

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
}