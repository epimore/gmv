use std::time::Duration;

use base::tokio::sync::mpsc;
use base::tokio_util::sync::CancellationToken;
use base_rpc::RetryPolicy;

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::executor::MqttCommandExecutor;
use crate::mqttc::publisher::MqttPublisher;
use crate::mqttc::subscriber::{CommandIdRepository, MqttCommandPolicy};
use crate::store::persistent::IntegrationRepository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttProtocolVersion {
    V3,
    V5,
}

impl MqttProtocolVersion {
    pub fn parse(value: &str) -> GuardResult<Self> {
        match value {
            "v3" => Ok(Self::V3),
            "v5" => Ok(Self::V5),
            _ => Err(GuardError::InvalidConfig(
                "MQTT protocol version must be v3 or v5".to_string(),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::V3 => "v3",
            Self::V5 => "v5",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MqttClientConfig {
    pub protocol_version: MqttProtocolVersion,
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

    fn common(&self) -> base_mqtt::MqttClientConfig {
        base_mqtt::MqttClientConfig {
            protocol_version: match self.protocol_version {
                MqttProtocolVersion::V3 => base_mqtt::MqttProtocolVersion::V3,
                MqttProtocolVersion::V5 => base_mqtt::MqttProtocolVersion::V5,
            },
            client_id: self.client_id.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self
                .password
                .as_ref()
                .map(|password| password.expose().to_string()),
            keep_alive: self.keep_alive,
            request_capacity: self.request_capacity,
            clean_start: true,
            session_expiry: None,
            tls: self.tls.then_some(base_mqtt::MqttTlsConfig {
                ca_certificate_path: None,
                client_certificate_path: None,
                client_private_key_path: None,
            }),
            last_will: None,
            reconnect: base_mqtt::MqttReconnectPolicy {
                initial_delay: self.retry.initial_delay,
                max_delay: self.retry.max_delay,
                multiplier: self.retry.multiplier,
                jitter_ratio: self.retry.jitter_ratio,
                max_attempts: self.retry.max_attempts,
            },
        }
    }
}

pub struct MqttRuntime {
    runtime: Option<base_mqtt::MqttRuntime>,
    pub publisher: MqttPublisher,
}

impl MqttRuntime {
    pub fn new(config: MqttClientConfig) -> GuardResult<Self> {
        config.validate()?;
        let runtime = base_mqtt::MqttRuntime::new(config.common(), Vec::new())
            .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
        let publisher = MqttPublisher::new(runtime.publisher(), config.retry);
        Ok(Self {
            runtime: Some(runtime),
            publisher,
        })
    }

    pub async fn run(self, cancel: CancellationToken) -> GuardResult<()> {
        self.run_loop(cancel, None, None).await
    }

    pub async fn run_with_ready(
        self,
        cancel: CancellationToken,
        ready: base::tokio::sync::oneshot::Sender<()>,
    ) -> GuardResult<()> {
        self.run_loop(cancel, None, Some(ready)).await
    }

    pub async fn run_commands(
        self,
        topics: Vec<String>,
        policy: MqttCommandPolicy,
        repository: CommandIdRepository,
        executor: MqttCommandExecutor,
        integrations: IntegrationRepository,
        cancel: CancellationToken,
    ) -> GuardResult<()> {
        self.run_commands_with_ready(
            topics,
            policy,
            repository,
            executor,
            integrations,
            cancel,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_commands_with_ready(
        mut self,
        topics: Vec<String>,
        policy: MqttCommandPolicy,
        repository: CommandIdRepository,
        executor: MqttCommandExecutor,
        integrations: IntegrationRepository,
        cancel: CancellationToken,
        ready: Option<base::tokio::sync::oneshot::Sender<()>>,
    ) -> GuardResult<()> {
        if topics.is_empty() {
            return Err(GuardError::InvalidConfig(
                "MQTT subscribe_topics is required when command subscription is enabled"
                    .to_string(),
            ));
        }
        let subscriptions = topics
            .into_iter()
            .map(|topic_filter| base_mqtt::MqttSubscription {
                topic_filter,
                qos: base_mqtt::MqttQos::AtLeastOnce,
            })
            .collect();
        self.runtime
            .as_mut()
            .expect("Guard MQTT runtime is initialized")
            .set_subscriptions(subscriptions)
            .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
        self.run_loop(
            cancel,
            Some(CommandRuntime {
                policy,
                repository,
                executor,
                integrations,
            }),
            ready,
        )
        .await
    }

    async fn run_loop(
        mut self,
        cancel: CancellationToken,
        mut commands: Option<CommandRuntime>,
        mut ready: Option<base::tokio::sync::oneshot::Sender<()>>,
    ) -> GuardResult<()> {
        let runtime = self
            .runtime
            .take()
            .expect("Guard MQTT runtime is initialized");
        let (events_tx, mut events_rx) = mpsc::channel(256);
        let driver_cancel = cancel.child_token();
        let driver = base::tokio::spawn(runtime.run(driver_cancel.clone(), events_tx));
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => break,
                event = events_rx.recv() => match event {
                    Some(base_mqtt::MqttEvent::Connected) => {
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(());
                        }
                    }
                    Some(base_mqtt::MqttEvent::Disconnected(reason)) => {
                        base::log::warn!("MQTT runtime disconnected: {reason}");
                    }
                    Some(base_mqtt::MqttEvent::Publish { topic, payload, .. }) => {
                        if let Some(commands) = commands.as_mut()
                            && let Err(error) = commands.handle(&topic, &payload).await
                        {
                            base::log::warn!("MQTT command rejected: topic={topic}, reason={error}");
                        }
                    }
                    None => break,
                }
            }
        }
        driver_cancel.cancel();
        match driver.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(GuardError::Conflict(format!(
                "MQTT event loop failed: {error}"
            ))),
            Err(error) => Err(GuardError::Conflict(format!(
                "MQTT event loop join failed: {error}"
            ))),
        }
    }
}

struct CommandRuntime {
    policy: MqttCommandPolicy,
    repository: CommandIdRepository,
    executor: MqttCommandExecutor,
    integrations: IntegrationRepository,
}

impl CommandRuntime {
    async fn handle(&mut self, topic: &str, payload: &[u8]) -> GuardResult<()> {
        let now_ms = now_ms();
        if let Some(command) = self
            .policy
            .decode_authorized_topic_with_repository(
                topic,
                payload,
                now_ms,
                &self.repository,
                &self.integrations,
            )
            .await?
        {
            base::log::info!(
                "MQTT command accepted: action=mqtt_command, stage=claim, outcome=accepted, command_id={}, integration_id={}, command_action={}, target={}, topic={}, payload_bytes={}",
                command.command_id,
                command.integration_id,
                command.action.as_str(),
                command.target,
                topic,
                payload.len()
            );
            let command_id = command.command_id.clone();
            let result = self.executor.execute(command).await;
            self.repository
                .complete(&command_id, result.is_ok(), now_ms)
                .await?;
            result?;
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mqtt_protocol_version_is_explicit() {
        assert_eq!(MqttProtocolVersion::parse("v3").unwrap().as_str(), "v3");
        assert_eq!(MqttProtocolVersion::parse("v5").unwrap().as_str(), "v5");
        assert!(MqttProtocolVersion::parse("v4").is_err());
    }
}
