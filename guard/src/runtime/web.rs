use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::api::v2::http::HttpState;
use crate::app_config::GuardAppConfig;
use crate::auth::{AuthState, SessionPolicy, UserAccount};
use crate::core::{GuardError, GuardResult};
use crate::runtime::application_router;

#[derive(Debug, Clone)]
pub struct WebTlsConfig {
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WebServerConfig {
    pub bind_addr: SocketAddr,
    pub allowed_origin: String,
    pub ui_dist_dir: PathBuf,
    pub tls: Option<WebTlsConfig>,
    pub simulator_enabled: bool,
    pub session_ttl: Duration,
    pub login_window: Duration,
    pub max_failed_attempts: usize,
}

impl WebServerConfig {
    pub fn from_app(config: &GuardAppConfig) -> GuardResult<Self> {
        config.validate()?;
        let http = &config.http;
        let result = Self {
            bind_addr: http.bind_addr,
            allowed_origin: http.allowed_origin.clone(),
            ui_dist_dir: http.ui_dist_dir.clone(),
            tls: http.tls.enabled.then(|| WebTlsConfig {
                certificate_path: http.tls.certificate_path.clone(),
                private_key_path: http.tls.private_key_path.clone(),
            }),
            simulator_enabled: config.simulator.enabled,
            session_ttl: Duration::from_secs(http.session_ttl_sec),
            login_window: Duration::from_secs(http.login_window_sec),
            max_failed_attempts: http.max_failed_attempts,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> GuardResult<()> {
        if self.tls.is_none() && !self.bind_addr.ip().is_loopback() {
            return Err(GuardError::InvalidConfig(
                "TLS can only be disabled on a loopback HTTP bind".to_string(),
            ));
        }
        if self
            .allowed_origin
            .parse::<axum::http::HeaderValue>()
            .is_err()
        {
            return Err(GuardError::InvalidConfig(
                "guard.http.allowed_origin must be a valid Origin".to_string(),
            ));
        }
        Ok(())
    }
}

pub async fn serve(
    config: WebServerConfig,
    api: crate::api::v2::ApiV2,
    outbox: crate::outbox::OutboxRepository,
    simulator: Option<crate::sim::Simulator>,
    users: Vec<UserAccount>,
) -> Result<(), Box<dyn std::error::Error>> {
    config.validate()?;
    let auth = AuthState::new(
        users,
        SessionPolicy {
            allowed_origin: config.allowed_origin.clone(),
            secure_cookie: config.tls.is_some(),
            session_ttl: config.session_ttl,
            login_window: config.login_window,
            max_failed_attempts: config.max_failed_attempts,
        },
    );
    let app = application_router(
        HttpState {
            api,
            auth,
            outbox,
            simulator,
        },
        config.ui_dist_dir.clone(),
    );
    if let Some(tls) = config.tls {
        let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            tls.certificate_path,
            tls.private_key_path,
        )
        .await?;
        axum_server::bind_rustls(config.bind_addr, rustls)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = base::tokio::net::TcpListener::bind(config.bind_addr).await?;
        axum::serve(listener, app).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(bind_addr: &str, tls: bool) -> WebServerConfig {
        WebServerConfig {
            bind_addr: bind_addr.parse().unwrap(),
            allowed_origin: "http://127.0.0.1:8080".to_string(),
            ui_dist_dir: PathBuf::from("guard-ui/dist"),
            tls: tls.then(|| WebTlsConfig {
                certificate_path: PathBuf::from("server.pem"),
                private_key_path: PathBuf::from("server-key.pem"),
            }),
            simulator_enabled: false,
            session_ttl: Duration::from_secs(3600),
            login_window: Duration::from_secs(60),
            max_failed_attempts: 5,
        }
    }

    #[test]
    fn rejects_plain_http_on_non_loopback_bind() {
        assert!(config("0.0.0.0:8080", false).validate().is_err());
        config("127.0.0.1:8080", false).validate().unwrap();
    }

    #[test]
    fn accepts_tls_on_non_loopback_bind() {
        config("0.0.0.0:8443", true).validate().unwrap();
    }
}
