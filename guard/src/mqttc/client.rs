use std::time::Duration;

use base::tokio_util::sync::CancellationToken;
use base_rpc::RetryPolicy;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, Transport};

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::publisher::MqttPublisher;

#[derive(Debug, Clone)]
pub struct MqttClientConfig {
    pub client_id: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub keep_alive: Duration,
    pub request_capacity: usize,
    pub tls: bool,
    pub retry: RetryPolicy,
}

impl MqttClientConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if self.client_id.is_empty() || self.host.is_empty() || self.port == 0 {
            return Err(GuardError::InvalidConfig(
                "MQTT client_id, host, and port are required".to_string(),
            ));
        }
        if self.request_capacity == 0 || self.keep_alive.is_zero() {
            return Err(GuardError::InvalidConfig(
                "MQTT request capacity and keep alive must be positive".to_string(),
            ));
        }
        if self.username.is_some() != self.password.is_some() {
            return Err(GuardError::InvalidConfig(
                "MQTT username and password must be configured together".to_string(),
            ));
        }
        Ok(())
    }
}

pub struct MqttRuntime {
    pub publisher: MqttPublisher,
    event_loop: EventLoop,
}

impl MqttRuntime {
    pub fn new(config: MqttClientConfig) -> GuardResult<Self> {
        config.validate()?;
        let mut options = MqttOptions::new(config.client_id, config.host, config.port);
        options.set_keep_alive(config.keep_alive);
        if config.tls {
            options.set_transport(Transport::tls_with_default_config());
        }
        if let (Some(username), Some(password)) = (config.username, config.password) {
            options.set_credentials(username, password.expose());
        }
        let (client, event_loop) = AsyncClient::new(options, config.request_capacity);
        Ok(Self {
            publisher: MqttPublisher::new(client, config.retry),
            event_loop,
        })
    }

    pub async fn run(mut self, cancel: CancellationToken) -> GuardResult<()> {
        let mut attempt = 0;
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                event = self.event_loop.poll() => match event {
                    Ok(_) => attempt = 0,
                    Err(error) => {
                        attempt += 1;
                        if !self.publisher.retry_policy().permits(attempt) {
                            return Err(GuardError::Conflict(format!("MQTT event loop failed: {error}")));
                        }
                        let delay = self.publisher.retry_policy().delay(attempt);
                        base::tokio::select! {
                            _ = cancel.cancelled() => return Ok(()),
                            _ = base::tokio::time::sleep(delay) => {}
                        }
                    }
                }
            }
        }
    }
}
