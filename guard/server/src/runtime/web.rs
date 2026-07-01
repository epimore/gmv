use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::time::Duration;

use crate::api::v2::http::HttpState;
use crate::app_config::GuardAppConfig;
use crate::auth::{AuthState, SessionPolicy, UserAccount};
use crate::core::{GuardError, GuardResult};
use crate::runtime::application_router;
use crate::runtime::event_forwarder::EventForwarder;

#[derive(Debug, Clone)]
pub struct WebTlsConfig {
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WebServerConfig {
    pub bind_addr: SocketAddr,
    pub allowed_origins: Vec<String>,
    pub ui_dist_dir: PathBuf,
    pub tls: Option<WebTlsConfig>,
    pub session_ttl: Duration,
    pub login_window: Duration,
    pub max_failed_attempts: usize,
    pub local_admin_username: String,
    pub local_admin_login_only: bool,
}

impl WebServerConfig {
    pub fn from_app(config: &GuardAppConfig) -> GuardResult<Self> {
        config.validate()?;
        let http = &config.http;
        let result = Self {
            bind_addr: http.bind_addr,
            allowed_origins: http.origins(),
            ui_dist_dir: http.ui_dist_dir.clone(),
            tls: http.tls.enabled.then(|| WebTlsConfig {
                certificate_path: http.tls.certificate_path.clone(),
                private_key_path: http.tls.private_key_path.clone(),
            }),
            session_ttl: Duration::from_secs(http.session_ttl_sec),
            login_window: Duration::from_secs(http.login_window_sec),
            max_failed_attempts: http.max_failed_attempts,
            local_admin_username: config.bootstrap.admin.username.clone(),
            local_admin_login_only: config.bootstrap.admin.local_login_only,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> GuardResult<()> {
        if self.allowed_origins.is_empty() {
            return Err(GuardError::InvalidConfig(
                "guard.http.origins must not be empty".to_string(),
            ));
        }
        for origin in &self.allowed_origins {
            if origin.parse::<axum::http::HeaderValue>().is_err() {
                return Err(GuardError::InvalidConfig(format!(
                    "guard.http.origins contains an invalid Origin: {origin}"
                )));
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    config: WebServerConfig,
    listener: StdTcpListener,
    api: crate::api::v2::ApiV2,
    outbox: crate::outbox::OutboxRepository,
    users: Vec<UserAccount>,
    user_repository: crate::store::persistent::UserRepository,
    event_forwarder: Option<EventForwarder>,
) -> Result<(), Box<dyn std::error::Error>> {
    config.validate()?;
    base::log::debug!(
        "guard http service inbound: bind_addr={}, tls={}",
        config.bind_addr,
        config.tls.is_some()
    );
    let auth = AuthState::new(
        users,
        SessionPolicy {
            allowed_origins: config.allowed_origins.clone(),
            secure_cookie: config.tls.is_some(),
            session_ttl: config.session_ttl,
            login_window: config.login_window,
            max_failed_attempts: config.max_failed_attempts,
            local_admin_username: Some(config.local_admin_username.clone()),
            local_admin_login_only: config.local_admin_login_only,
        },
    );
    let app = application_router(
        HttpState {
            api,
            auth,
            outbox,
            users: Some(user_repository),
            event_forwarder,
        },
        config.ui_dist_dir.clone(),
    );
    listener.set_nonblocking(true)?;
    if let Some(tls) = config.tls {
        let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            tls.certificate_path,
            tls.private_key_path,
        )
        .await?;
        axum_server::from_tcp_rustls(listener, rustls)?
            .serve(app.into_make_service())
            .await?;
    } else {
        axum_server::from_tcp(listener)?
            .serve(app.into_make_service())
            .await?;
    }
    base::log::debug!(
        "guard http service outbound: bind_addr={}",
        config.bind_addr
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(bind_addr: &str, tls: bool) -> WebServerConfig {
        WebServerConfig {
            bind_addr: bind_addr.parse().unwrap(),
            allowed_origins: vec!["http://127.0.0.1:8080".to_string()],
            ui_dist_dir: PathBuf::from("guard/ui/dist"),
            tls: tls.then(|| WebTlsConfig {
                certificate_path: PathBuf::from("server.pem"),
                private_key_path: PathBuf::from("server-key.pem"),
            }),
            session_ttl: Duration::from_secs(3600),
            login_window: Duration::from_secs(60),
            max_failed_attempts: 5,
            local_admin_username: "admin".to_string(),
            local_admin_login_only: true,
        }
    }

    #[test]
    fn accepts_plain_http_on_non_loopback_bind() {
        config("0.0.0.0:8080", false).validate().unwrap();
        config("127.0.0.1:8080", false).validate().unwrap();
    }

    #[test]
    fn accepts_tls_on_non_loopback_bind() {
        config("0.0.0.0:8443", true).validate().unwrap();
    }

    #[test]
    fn rejects_empty_origins() {
        let mut config = config("127.0.0.1:8080", false);
        config.allowed_origins.clear();
        assert!(config.validate().is_err());
    }
}
