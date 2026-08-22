use std::time::Duration;

use base::tokio::task::JoinHandle;
use base::tokio_util::sync::CancellationToken;

use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::integration::model::{
    Integration, IntegrationTransport, MqttRuntimeApplyState, MqttRuntimeConfig,
    MqttRuntimeRevision,
};
use crate::integration::secret::IntegrationSecretManager;
use crate::mqttc::{
    CommandIdRepository, MqttClientConfig, MqttCommandExecutor, MqttCommandPolicy,
    MqttProtocolVersion, MqttPublisher, MqttRuntime,
};
use crate::store::persistent::IntegrationRepository;

struct ActiveRuntime {
    revision: i64,
    cancel: CancellationToken,
    task: JoinHandle<GuardResult<()>>,
}

pub struct MqttRuntimeManager {
    integrations: IntegrationRepository,
    command_ids: CommandIdRepository,
    executor: MqttCommandExecutor,
    secrets: Option<IntegrationSecretManager>,
    publisher: MqttPublisher,
}

impl MqttRuntimeManager {
    pub fn new(
        integrations: IntegrationRepository,
        command_ids: CommandIdRepository,
        executor: MqttCommandExecutor,
        secrets: Option<IntegrationSecretManager>,
    ) -> Self {
        Self {
            integrations,
            command_ids,
            executor,
            secrets,
            publisher: MqttPublisher::disconnected(base_rpc::RetryPolicy::default()),
        }
    }

    pub fn publisher(&self) -> MqttPublisher {
        self.publisher.clone()
    }

    pub async fn run(self, cancel: CancellationToken) -> GuardResult<()> {
        let mut active: Option<ActiveRuntime> = None;
        let mut retry_after_ms = 0_i64;
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => {
                    stop_active(&mut active).await?;
                    self.publisher.disconnect();
                    return Ok(());
                }
                _ = base::tokio::time::sleep(Duration::from_millis(500)) => {}
            }

            if active
                .as_ref()
                .is_some_and(|runtime| runtime.task.is_finished())
            {
                let stopped = active.take().expect("active MQTT runtime must exist");
                let revision = stopped.revision;
                let result = stopped.task.await;
                self.publisher.disconnect();
                base::log::warn!(
                    "MQTT runtime ended: action=mqtt_runtime, stage=run, outcome=degraded, revision={}, reason={}",
                    revision,
                    if matches!(result, Ok(Ok(()))) {
                        "unexpected_stop"
                    } else {
                        "connection_lost"
                    }
                );
                if let Some(config) = self.integrations.mqtt_runtime_config().await? {
                    self.integrations
                        .update_mqtt_runtime_state(
                            config.desired_revision,
                            config.active_revision,
                            MqttRuntimeApplyState::Degraded,
                            Some("mqtt_runtime_stopped"),
                            Some("MQTT connection stopped and will be retried"),
                            now_ms(),
                        )
                        .await?;
                }
            }

            let integration = self.integrations.business_integration().await?;
            let config = self.integrations.mqtt_runtime_config().await?;
            let should_run = integration.as_ref().is_some_and(|value| {
                value.enabled
                    && value.transport == IntegrationTransport::Mqtt
                    && (value.inbound_enabled || value.outbound_enabled)
            });
            if !should_run {
                stop_active(&mut active).await?;
                self.publisher.disconnect();
                if let Some(config) = config
                    && config.apply_state != MqttRuntimeApplyState::Disabled
                {
                    self.integrations
                        .update_mqtt_runtime_state(
                            config.desired_revision,
                            config.active_revision,
                            MqttRuntimeApplyState::Disabled,
                            None,
                            None,
                            now_ms(),
                        )
                        .await?;
                }
                continue;
            }
            let Some(config) = config else {
                continue;
            };
            if active
                .as_ref()
                .is_some_and(|runtime| runtime.revision == config.desired_revision)
                && config.apply_state == MqttRuntimeApplyState::Connected
            {
                continue;
            }
            if active.as_ref().is_some_and(|runtime| {
                config.apply_state == MqttRuntimeApplyState::Degraded
                    && config.active_revision == Some(runtime.revision)
            }) || (active.is_none()
                && config.apply_state == MqttRuntimeApplyState::Degraded
                && now_ms() < retry_after_ms)
            {
                continue;
            }
            let integration = integration.expect("enabled MQTT integration must exist");
            stop_active(&mut active).await?;
            self.publisher.disconnect();
            self.integrations
                .update_mqtt_runtime_state(
                    config.desired_revision,
                    config.active_revision,
                    MqttRuntimeApplyState::Applying,
                    None,
                    None,
                    now_ms(),
                )
                .await?;

            match self
                .start_revision(&integration, config.desired_revision)
                .await
            {
                Ok((runtime, candidate)) => {
                    retry_after_ms = 0;
                    self.publisher.replace_from(&candidate);
                    self.integrations
                        .update_mqtt_runtime_state(
                            config.desired_revision,
                            Some(config.desired_revision),
                            MqttRuntimeApplyState::Connected,
                            None,
                            None,
                            now_ms(),
                        )
                        .await?;
                    base::log::info!(
                        "MQTT runtime applied: action=mqtt_runtime, stage=apply, outcome=connected, revision={}",
                        config.desired_revision
                    );
                    active = Some(runtime);
                }
                Err(error) => {
                    retry_after_ms = now_ms().saturating_add(30_000);
                    base::log::warn!(
                        "MQTT runtime apply failed: action=mqtt_runtime, stage=apply, outcome=failed, revision={}, reason=connection_failed",
                        config.desired_revision
                    );
                    active = self
                        .restore_active_revision(&integration, &config, &error)
                        .await?;
                }
            }
        }
    }

    async fn restore_active_revision(
        &self,
        integration: &Integration,
        config: &MqttRuntimeConfig,
        _apply_error: &GuardError,
    ) -> GuardResult<Option<ActiveRuntime>> {
        let Some(active_revision) = config
            .active_revision
            .filter(|revision| *revision != config.desired_revision)
        else {
            self.integrations
                .update_mqtt_runtime_state(
                    config.desired_revision,
                    None,
                    MqttRuntimeApplyState::Degraded,
                    Some("mqtt_connect_failed"),
                    Some("Unable to establish the MQTT connection"),
                    now_ms(),
                )
                .await?;
            return Ok(None);
        };
        self.integrations
            .update_mqtt_runtime_state(
                config.desired_revision,
                Some(active_revision),
                MqttRuntimeApplyState::RollingBack,
                Some("mqtt_connect_failed"),
                Some("New MQTT configuration failed; restoring the active revision"),
                now_ms(),
            )
            .await?;
        match self.start_revision(integration, active_revision).await {
            Ok((runtime, publisher)) => {
                self.publisher.replace_from(&publisher);
                self.integrations
                    .update_mqtt_runtime_state(
                        config.desired_revision,
                        Some(active_revision),
                        MqttRuntimeApplyState::Degraded,
                        Some("mqtt_connect_failed"),
                        Some("New MQTT configuration failed; the previous revision is active"),
                        now_ms(),
                    )
                    .await?;
                Ok(Some(runtime))
            }
            Err(_) => {
                self.integrations
                    .update_mqtt_runtime_state(
                        config.desired_revision,
                        None,
                        MqttRuntimeApplyState::Degraded,
                        Some("mqtt_rollback_failed"),
                        Some("MQTT configuration and rollback both failed"),
                        now_ms(),
                    )
                    .await?;
                Ok(None)
            }
        }
    }

    async fn start_revision(
        &self,
        integration: &Integration,
        revision: i64,
    ) -> GuardResult<(ActiveRuntime, MqttPublisher)> {
        let value = self
            .integrations
            .mqtt_runtime_revision(revision)
            .await?
            .ok_or_else(|| GuardError::Conflict(format!("MQTT revision {revision} is missing")))?;
        let runtime = MqttRuntime::new(self.client_config(&value).await?)?;
        let publisher = runtime.publisher.clone();
        let child_cancel = CancellationToken::new();
        let task_cancel = child_cancel.clone();
        let (ready_tx, ready_rx) = base::tokio::sync::oneshot::channel();
        let task = if integration.inbound_enabled {
            let mqtt = self
                .integrations
                .mqtt_config(&integration.integration_id)
                .await?
                .ok_or_else(|| {
                    GuardError::InvalidConfig("MQTT integration config missing".to_string())
                })?;
            let policy = MqttCommandPolicy::new(
                crate::integration::model::MQTT_COMMAND_ACTIONS
                    .iter()
                    .copied()
                    .map(str::to_string),
                300_000,
            )?;
            let command_ids = self.command_ids.clone();
            let executor = self.executor.clone();
            let integrations = self.integrations.clone();
            base::tokio::spawn(async move {
                runtime
                    .run_commands_with_ready(
                        vec![mqtt.command_topic],
                        policy,
                        command_ids,
                        executor,
                        integrations,
                        task_cancel,
                        Some(ready_tx),
                    )
                    .await
            })
        } else {
            base::tokio::spawn(async move { runtime.run_with_ready(task_cancel, ready_tx).await })
        };
        match base::tokio::time::timeout(Duration::from_secs(10), ready_rx).await {
            Ok(Ok(())) => Ok((
                ActiveRuntime {
                    revision,
                    cancel: child_cancel,
                    task,
                },
                publisher,
            )),
            Ok(Err(_)) => {
                child_cancel.cancel();
                match join_runtime_task(task).await {
                    Ok(()) => Err(GuardError::Conflict(
                        "MQTT connection stopped before readiness".to_string(),
                    )),
                    Err(error) => Err(error),
                }
            }
            Err(_) => {
                child_cancel.cancel();
                join_runtime_task(task).await?;
                Err(GuardError::Conflict(
                    "MQTT connection readiness timed out".to_string(),
                ))
            }
        }
    }

    async fn client_config(&self, value: &MqttRuntimeRevision) -> GuardResult<MqttClientConfig> {
        let password = match value.password_ciphertext.as_deref() {
            Some(ciphertext) => Some(Secret::new(
                self.secrets
                    .as_ref()
                    .ok_or_else(|| {
                        GuardError::Conflict("integration master key is unavailable".to_string())
                    })?
                    .decrypt(ciphertext)
                    .await?,
            )),
            None => None,
        };
        Ok(MqttClientConfig {
            protocol_version: MqttProtocolVersion::parse(&value.protocol_version)?,
            client_id: value.client_id.clone(),
            host: value.broker.clone(),
            port: value.port,
            username: value.username.clone(),
            password,
            keep_alive: Duration::from_secs(30),
            request_capacity: 100,
            tls: value.tls,
            retry: base_rpc::RetryPolicy::default(),
        })
    }
}

async fn stop_active(active: &mut Option<ActiveRuntime>) -> GuardResult<()> {
    let Some(runtime) = active.take() else {
        return Ok(());
    };
    runtime.cancel.cancel();
    join_runtime_task(runtime.task).await
}

async fn join_runtime_task(task: JoinHandle<GuardResult<()>>) -> GuardResult<()> {
    match task.await {
        Ok(result) => result,
        Err(error) => Err(GuardError::Conflict(format!(
            "MQTT runtime task join failed: {error}"
        ))),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
