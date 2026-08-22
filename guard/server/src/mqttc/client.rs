use std::time::Duration;

use base::tokio_util::sync::CancellationToken;
use base_rpc::RetryPolicy;
use rumqttc::v5::mqttbytes::QoS as QoSV5;
use rumqttc::v5::{
    AsyncClient as AsyncClientV5, Event as EventV5, EventLoop as EventLoopV5,
    MqttOptions as MqttOptionsV5,
};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, Transport};

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::executor::MqttCommandExecutor;
use crate::mqttc::publisher::{MqttPublishClient, MqttPublisher};
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
}

enum MqttClient {
    V3(AsyncClient),
    V5(AsyncClientV5),
}

enum MqttEventLoop {
    V3(Box<EventLoop>),
    V5(Box<EventLoopV5>),
}

pub struct MqttRuntime {
    pub publisher: MqttPublisher,
    client: MqttClient,
    event_loop: MqttEventLoop,
}

impl MqttRuntime {
    pub fn new(config: MqttClientConfig) -> GuardResult<Self> {
        config.validate()?;
        let (client, event_loop, publish_client) = match config.protocol_version {
            MqttProtocolVersion::V3 => {
                let mut options = MqttOptions::new(&config.client_id, &config.host, config.port);
                options.set_keep_alive(config.keep_alive);
                if config.tls {
                    options.set_transport(Transport::tls_with_default_config());
                }
                if let (Some(username), Some(password)) = (&config.username, &config.password) {
                    options.set_credentials(username, password.expose());
                }
                let (client, event_loop) = AsyncClient::new(options, config.request_capacity);
                (
                    MqttClient::V3(client.clone()),
                    MqttEventLoop::V3(Box::new(event_loop)),
                    MqttPublishClient::V3(client),
                )
            }
            MqttProtocolVersion::V5 => {
                let mut options = MqttOptionsV5::new(&config.client_id, &config.host, config.port);
                options.set_keep_alive(config.keep_alive);
                if config.tls {
                    options.set_transport(Transport::tls_with_default_config());
                }
                if let (Some(username), Some(password)) = (&config.username, &config.password) {
                    options.set_credentials(username, password.expose());
                }
                let (client, event_loop) = AsyncClientV5::new(options, config.request_capacity);
                (
                    MqttClient::V5(client.clone()),
                    MqttEventLoop::V5(Box::new(event_loop)),
                    MqttPublishClient::V5(client),
                )
            }
        };
        Ok(Self {
            publisher: MqttPublisher::new(publish_client, config.retry),
            client,
            event_loop,
        })
    }

    pub async fn run(mut self, cancel: CancellationToken) -> GuardResult<()> {
        self.run_loop(cancel, None, None).await
    }

    pub async fn run_with_ready(
        mut self,
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
        for topic in &topics {
            match &self.client {
                MqttClient::V3(client) => {
                    client
                        .subscribe(topic, QoS::AtLeastOnce)
                        .await
                        .map_err(|error| {
                            GuardError::Conflict(format!(
                                "MQTT v3 subscribe {topic} failed: {error}"
                            ))
                        })?
                }
                MqttClient::V5(client) => client
                    .subscribe(topic, QoSV5::AtLeastOnce)
                    .await
                    .map_err(|error| {
                        GuardError::Conflict(format!("MQTT v5 subscribe {topic} failed: {error}"))
                    })?,
            }
        }
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
        &mut self,
        cancel: CancellationToken,
        mut commands: Option<CommandRuntime>,
        mut ready: Option<base::tokio::sync::oneshot::Sender<()>>,
    ) -> GuardResult<()> {
        let mut attempt = 0;
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                event = poll_event(&mut self.event_loop) => match event {
                    Ok(event) => {
                        attempt = 0;
                        if event.is_connected()
                            && let Some(ready) = ready.take()
                        {
                            let _ = ready.send(());
                        }
                        if let (Some(commands), Some((topic, payload))) = (commands.as_mut(), event.publish())
                            && let Err(error) = commands.handle(topic, payload).await
                        {
                            base::log::warn!("MQTT command rejected: topic={topic}, reason={error}");
                        }
                    }
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

enum IncomingEvent {
    V3(Event),
    V5(Box<EventV5>),
}

impl IncomingEvent {
    fn is_connected(&self) -> bool {
        match self {
            Self::V3(Event::Incoming(Packet::ConnAck(_))) => true,
            Self::V5(event) => matches!(
                event.as_ref(),
                EventV5::Incoming(rumqttc::v5::mqttbytes::v5::Packet::ConnAck(_))
            ),
            _ => false,
        }
    }

    fn publish(&self) -> Option<(&str, &[u8])> {
        match self {
            Self::V3(Event::Incoming(Packet::Publish(publish))) => {
                Some((&publish.topic, &publish.payload))
            }
            Self::V5(event) => match event.as_ref() {
                EventV5::Incoming(rumqttc::v5::mqttbytes::v5::Packet::Publish(publish)) => {
                    Some((std::str::from_utf8(&publish.topic).ok()?, &publish.payload))
                }
                _ => None,
            },
            _ => None,
        }
    }
}

async fn poll_event(event_loop: &mut MqttEventLoop) -> Result<IncomingEvent, String> {
    match event_loop {
        MqttEventLoop::V3(event_loop) => event_loop
            .poll()
            .await
            .map(IncomingEvent::V3)
            .map_err(|error| error.to_string()),
        MqttEventLoop::V5(event_loop) => event_loop
            .poll()
            .await
            .map(|event| IncomingEvent::V5(Box::new(event)))
            .map_err(|error| error.to_string()),
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
