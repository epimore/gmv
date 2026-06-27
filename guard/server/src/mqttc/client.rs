use std::time::Duration;

use base::tokio_util::sync::CancellationToken;
use base_rpc::RetryPolicy;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, Transport};

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::executor::MqttCommandExecutor;
use crate::mqttc::publisher::MqttPublisher;
use crate::mqttc::subscriber::{CommandIdRepository, MqttCommandPolicy};

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
    client: AsyncClient,
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
            publisher: MqttPublisher::new(client.clone(), config.retry),
            client,
            event_loop,
        })
    }

    pub async fn run(mut self, cancel: CancellationToken) -> GuardResult<()> {
        self.run_loop(cancel, None).await
    }

    pub async fn run_commands(
        mut self,
        topics: Vec<String>,
        policy: MqttCommandPolicy,
        repository: CommandIdRepository,
        executor: MqttCommandExecutor,
        cancel: CancellationToken,
    ) -> GuardResult<()> {
        if topics.is_empty() {
            return Err(GuardError::InvalidConfig(
                "MQTT subscribe_topics is required when command subscription is enabled"
                    .to_string(),
            ));
        }
        for topic in &topics {
            self.client
                .subscribe(topic, QoS::AtLeastOnce)
                .await
                .map_err(|error| {
                    GuardError::Conflict(format!("MQTT subscribe {topic} failed: {error}"))
                })?;
        }
        self.run_loop(
            cancel,
            Some(CommandRuntime {
                policy,
                repository,
                executor,
            }),
        )
        .await
    }

    async fn run_loop(
        &mut self,
        cancel: CancellationToken,
        mut commands: Option<CommandRuntime>,
    ) -> GuardResult<()> {
        let mut attempt = 0;
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                event = self.event_loop.poll() => match event {
                    Ok(event) => {
                        attempt = 0;
                        if let Some(commands) = commands.as_mut() {
                            commands.handle(event).await?;
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

struct CommandRuntime {
    policy: MqttCommandPolicy,
    repository: CommandIdRepository,
    executor: MqttCommandExecutor,
}

impl CommandRuntime {
    async fn handle(&mut self, event: Event) -> GuardResult<()> {
        let Event::Incoming(Packet::Publish(publish)) = event else {
            return Ok(());
        };
        let now_ms = now_ms();
        if let Some(command) = self
            .policy
            .decode_with_repository(&publish.payload, now_ms, &self.repository)
            .await?
        {
            self.executor.execute(command).await?;
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
