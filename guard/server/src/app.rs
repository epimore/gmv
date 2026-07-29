use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::logger;
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::tokio_util::sync::CancellationToken;
use sha2::{Digest, Sha256};

use crate::api::v2::ApiV2;
use crate::app_config::GuardAppConfig;
use crate::auth::{AuthState, Secret, SessionPolicy};
use crate::core::{GuardError, GuardResult};
use crate::mqttc::{
    CommandIdRepository, MqttClientConfig, MqttCommandExecutor, MqttCommandPolicy,
    MqttProtocolVersion, MqttRuntime,
};
use crate::operation::OperationService;
use crate::outbox::{DeliveryRouter, OutboxWorker};
use crate::runtime::event_forwarder::{EventForwardRule, EventForwarder};
use crate::runtime::node_expirer;
use crate::runtime::node_rpc::{self, NodeRpcConfig};
use crate::runtime::web::{self, WebServerConfig};
use crate::store::InMemoryGuardStore;
use crate::store::persistent::PersistentStore;

pub struct AppInfo {
    config: GuardAppConfig,
}

pub struct GuardListeners {
    web: TcpListener,
    rpc: TcpListener,
}

impl Daemon<GuardListeners> for AppInfo {
    fn cli_basic() -> CliBasic {
        default_cli_basic!()
    }

    fn init_privilege() -> GlobalResult<(Self, GuardListeners)>
    where
        Self: Sized,
    {
        logger::Logger::init()?;
        let config = GuardAppConfig::current();
        config
            .validate()
            .map_err(|error| global_error(format!("guard config invalid: {error}")))?;
        let web_config = WebServerConfig::from_app(&config)
            .map_err(|error| global_error(format!("guard web config invalid: {error}")))?;
        let web = TcpListener::bind(web_config.bind_addr).map_err(|error| {
            global_error(format!(
                "bind guard http {} failed: {error}",
                web_config.bind_addr
            ))
        })?;
        let rpc = TcpListener::bind(config.grpc.bind_addr).map_err(|error| {
            global_error(format!(
                "bind guard grpc {} failed: {error}",
                config.grpc.bind_addr
            ))
        })?;
        banner(
            Self::cli_basic().version,
            &web_config.bind_addr.to_string(),
            &config.grpc.bind_addr.to_string(),
            |msg| info!("{msg}"),
        );
        Ok((Self { config }, GuardListeners { web, rpc }))
    }

    fn run_app(self, listeners: GuardListeners) -> GlobalResult<()> {
        let runtime = base::tokio::runtime::Runtime::new().map_err(|err| {
            GlobalError::new_sys_error(
                &format!("create Guard tokio runtime failed: {err}"),
                |msg| error!("{msg}"),
            )
        })?;
        runtime
            .block_on(start_guard(self.config, listeners))
            .map_err(|err| {
                GlobalError::new_sys_error(&format!("Guard runtime failed: {err}"), |msg| {
                    error!("{msg}")
                })
            })?;
        Ok(())
    }
}

pub async fn start_guard(
    config: GuardAppConfig,
    listeners: GuardListeners,
) -> Result<(), Box<dyn std::error::Error>> {
    let web_config = WebServerConfig::from_app(&config)?;
    let persistent = PersistentStore::connect(&config).await?;
    persistent.initialize(&config).await?;
    let users = persistent.load_users().await?;
    let user_repository = persistent.user_repository();
    let integration_repository = persistent.integration_repository();
    let integration_master_key = config.integrations.master_key_value()?;
    let integration_secrets = integration_master_key
        .as_deref()
        .map(crate::integration::secret::IntegrationSecretCipher::from_base64_key_no_pad)
        .transpose()?;
    if integration_secrets.is_none()
        && integration_repository
            .list()
            .await?
            .iter()
            .any(|integration| {
                integration.enabled
                    && integration.transport
                        == crate::integration::model::IntegrationTransport::Http
            })
    {
        return Err(GuardError::InvalidConfig(format!(
            "{} is required while an HTTP integration is enabled",
            crate::integration::secret::INTEGRATION_MASTER_KEY_CONFIG
        ))
        .into());
    }
    for integration in integration_repository.list().await? {
        if integration.enabled
            && integration.transport == crate::integration::model::IntegrationTransport::Mqtt
            && !config.integrations.mqtt.enabled
        {
            return Err(GuardError::InvalidConfig(format!(
                "MQTT integration {} is enabled while MQTT runtime is disabled",
                integration.integration_id
            ))
            .into());
        }
        if integration.enabled
            && integration.transport == crate::integration::model::IntegrationTransport::Mqtt
            && let Some(mqtt) = integration_repository
                .mqtt_config(&integration.integration_id)
                .await?
            && mqtt.protocol_version != config.integrations.mqtt.protocol_version
        {
            return Err(GuardError::InvalidConfig(format!(
                "MQTT integration {} declares {}, but runtime uses {}",
                integration.integration_id,
                mqtt.protocol_version,
                config.integrations.mqtt.protocol_version
            ))
            .into());
        }
    }
    let store = InMemoryGuardStore::default();
    let auth = AuthState::new(
        users,
        SessionPolicy {
            allowed_origins: web_config.allowed_origins.clone(),
            secure_cookie: web_config.tls.is_some(),
            session_ttl: web_config.session_ttl,
            login_window: web_config.login_window,
            max_failed_attempts: web_config.max_failed_attempts,
            local_admin_username: Some(web_config.local_admin_username.clone()),
            local_admin_login_only: web_config.local_admin_login_only,
        },
    );
    let registry =
        crate::registry::RegistryService::with_policy(store.clone(), config.registry.to_policy());
    let api_store = store.clone();
    let operations = OperationService::default();
    let api = ApiV2::new(store, operations.clone());
    let rpc_config = NodeRpcConfig {
        bind_addr: config.grpc.bind_addr,
        heartbeat_interval_ms: config.grpc.heartbeat_interval_ms,
        heartbeat_timeout_ms: config.grpc.heartbeat_timeout_ms,
        tls: config.grpc.tls.enabled.then(|| node_rpc::NodeRpcTlsConfig {
            certificate_path: config.grpc.tls.certificate_path.clone(),
            private_key_path: config.grpc.tls.private_key_path.clone(),
        }),
    };
    let _node_expirer = node_expirer::spawn(registry.clone(), config.grpc.heartbeat_timeout_ms);
    let mqtt_publisher = if config.integrations.mqtt.enabled {
        Some(
            spawn_mqtt_runtime(
                &config,
                &persistent,
                operations.clone(),
                api_store.clone(),
                auth.clone(),
                &integration_repository,
            )
            .await?,
        )
    } else {
        None
    };
    let mqtt_rules = if config.integrations.mqtt.enabled {
        config
            .integrations
            .mqtt
            .publish_event_topics
            .iter()
            .map(|pattern| EventForwardRule {
                pattern: pattern.clone(),
                topic_prefix: config.integrations.mqtt.publish_topic_prefix.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let event_forwarder = Some(
        EventForwarder::new(persistent.outbox_repository(), mqtt_rules)
            .with_integrations(integration_repository.clone()),
    );
    if let Some(forwarder) = event_forwarder.clone() {
        spawn_integration_playback_renewal(api_store.clone(), forwarder);
    }
    let mut delivery = DeliveryRouter::default();
    if let Some(publisher) = mqtt_publisher {
        delivery = delivery.with(
            crate::store::model::OutboxDestinationKind::Mqtt,
            Arc::new(publisher),
        );
    }
    if let Some(cipher) = integration_secrets.clone() {
        delivery = delivery.with(
            crate::store::model::OutboxDestinationKind::Webhook,
            Arc::new(crate::webhook::IntegrationWebhookDelivery::new(
                integration_repository.clone(),
                cipher,
            )),
        );
    }
    spawn_outbox_worker(persistent.outbox_repository(), delivery);
    let web = web::serve(
        web_config,
        listeners.web,
        api,
        auth.clone(),
        persistent.outbox_repository(),
        user_repository,
        integration_repository,
        integration_secrets,
        config.integrations.mqtt.protocol_version.clone(),
        config.integrations.mqtt.enabled,
        event_forwarder.clone(),
    );
    let rpc = node_rpc::serve(
        rpc_config,
        listeners.rpc,
        registry,
        api_store.clone(),
        auth,
        event_forwarder,
    );
    base::tokio::try_join!(web, rpc).map(|_| ())
}

async fn spawn_mqtt_runtime(
    config: &GuardAppConfig,
    persistent: &PersistentStore,
    operations: OperationService,
    store: InMemoryGuardStore,
    auth: AuthState,
    integrations: &crate::store::persistent::IntegrationRepository,
) -> GuardResult<crate::mqttc::MqttPublisher> {
    let mqtt = &config.integrations.mqtt;
    let runtime = MqttRuntime::new(MqttClientConfig {
        protocol_version: MqttProtocolVersion::parse(&mqtt.protocol_version)?,
        client_id: mqtt.client_id.clone(),
        host: mqtt.broker.clone(),
        port: mqtt.port,
        username: Some(mqtt.username.clone()),
        password: Some(Secret::new(mqtt.password()?)),
        keep_alive: Duration::from_secs(30),
        request_capacity: 100,
        tls: mqtt.tls,
        retry: base_rpc::RetryPolicy::default(),
    })?;
    let publisher = runtime.publisher.clone();
    let mut topics = mqtt.subscribe_topics.clone();
    let mut topic_routes = Vec::new();
    let mut result_topics = std::collections::HashMap::new();
    for integration in integrations
        .list()
        .await?
        .into_iter()
        .filter(|integration| {
            integration.enabled
                && integration.inbound_enabled
                && integration.transport == crate::integration::model::IntegrationTransport::Mqtt
        })
    {
        if let Some(integration_mqtt) = integrations
            .mqtt_config(&integration.integration_id)
            .await?
        {
            topics.push(integration_mqtt.command_topic.clone());
            result_topics.insert(
                integration.integration_id.clone(),
                integration_mqtt.result_topic.clone(),
            );
            topic_routes.push((
                integration_mqtt.command_topic,
                integration.integration_id,
                integration_mqtt.allowed_actions,
            ));
        }
    }
    topics.sort();
    topics.dedup();
    let policy = MqttCommandPolicy::new(
        [
            "stream.start".to_string(),
            "stream.stop".to_string(),
            "device.ptz".to_string(),
            "ai.start".to_string(),
            "ai.cancel".to_string(),
            "playback.ticket.renew".to_string(),
        ],
        300_000,
    )?
    .with_topic_routes(topic_routes)?;
    let repository = match persistent {
        #[cfg(feature = "db-mysql")]
        PersistentStore::Mysql(store) => CommandIdRepository::from(store.clone()),
        #[cfg(feature = "db-sqlite")]
        PersistentStore::Sqlite(store) => CommandIdRepository::from(store.clone()),
    };
    let executor = MqttCommandExecutor::new(operations, store)
        .with_auth(auth)
        .with_result_outbox(persistent.outbox_repository(), result_topics);
    let cancel = CancellationToken::new();
    base::tokio::spawn(async move {
        let result = if topics.is_empty() {
            runtime.run(cancel).await
        } else {
            runtime
                .run_commands(topics, policy, repository, executor, cancel)
                .await
        };
        if let Err(error) = result {
            warn!("MQTT runtime stopped: {error}");
        }
    });
    Ok(publisher)
}

fn spawn_outbox_worker(repository: crate::outbox::OutboxRepository, delivery: DeliveryRouter) {
    let worker = OutboxWorker::new(
        repository,
        Arc::new(delivery),
        base_rpc::RetryPolicy::default(),
        100,
    )
    .with_delete_delivered(true);
    base::tokio::spawn(async move {
        let mut failure_episode = FailureEpisode::default();
        loop {
            match worker.run_once(now_ms().unwrap_or_default()).await {
                Ok(_) => {
                    if let EpisodeDecision::Recovered {
                        total,
                        suppressed,
                        duration,
                    } = failure_episode.record_success(Instant::now())
                    {
                        info!(
                            "integration outbox worker recovered: total_failures={total}, suppressed={suppressed}, duration_ms={}",
                            duration.as_millis()
                        );
                    }
                }
                Err(error) => match failure_episode.record_failure(Instant::now()) {
                    EpisodeDecision::Started => warn!("integration outbox worker failed: {error}"),
                    EpisodeDecision::Summary {
                        total,
                        suppressed,
                        duration,
                        ..
                    } => warn!(
                        "integration outbox worker remains failed: total={total}, suppressed={suppressed}, duration_ms={}",
                        duration.as_millis()
                    ),
                    EpisodeDecision::Suppressed => {}
                    EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
                },
            }
            base::tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

fn spawn_integration_playback_renewal(store: InMemoryGuardStore, forwarder: EventForwarder) {
    base::tokio::spawn(async move {
        loop {
            let now = now_ms().unwrap_or_default();
            for ticket in
                store.integration_playback_tickets_expiring_before(now, now.saturating_add(60_000))
            {
                let Some(integration_id) = ticket.username.strip_prefix("integration:") else {
                    continue;
                };
                let mut digest = Sha256::new();
                digest.update(ticket.token.as_bytes());
                digest.update(ticket.expires_at_ms.to_be_bytes());
                let event_id = format!("playback-renew-{}", hex::encode(digest.finalize()));
                let payload = match base::serde_json::to_vec(&base::serde_json::json!({
                    "token": ticket.token,
                    "playback_id": ticket.playback_id,
                    "stream_id": ticket.stream_id,
                    "output_id": ticket.output_id,
                    "subscription_id": ticket.subscription_id,
                    "expires_at_ms": ticket.expires_at_ms,
                    "response_action": "playback.ticket.renew"
                })) {
                    Ok(payload) => payload,
                    Err(error) => {
                        warn!("playback renewal request encode failed: {error}");
                        continue;
                    }
                };
                if let Err(error) = forwarder
                    .forward_for_integration(
                        integration_id,
                        event_id,
                        "integration.playback_ticket.renew_requested".to_string(),
                        payload,
                    )
                    .await
                {
                    warn!(
                        "playback renewal request enqueue failed: integration_id={integration_id}, reason={error}"
                    );
                }
            }
            base::tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

fn now_ms() -> GuardResult<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))?
        .as_millis()
        .min(i64::MAX as u128) as i64)
}

fn banner<F: FnOnce(String)>(version: &str, http_addr: &str, grpc_addr: &str, f: F) {
    let msg = format!(
        r#"
======================================================================
                [GMV:GUARD-SERVER]   Version: {}
======================================================================
┌──────────────────┬──────────────────────┬──────────────┬──────────────┐
│ Service          │ Address              │ Protocols    │  Status      │
├──────────────────┼──────────────────────┼──────────────┼──────────────┤
│ Guard HTTP       │ {:<20} │ HTTP         │ 🟢 Ready     │
│ Guard RPC        │ {:<20} │ gRPC         │ 🟢 Listening │
└──────────────────┴──────────────────────┴──────────────┴──────────────┘"#,
        version, http_addr, grpc_addr
    );
    f(msg);
}

fn global_error(message: String) -> GlobalError {
    GlobalError::new_sys_error(&message, |msg| error!("{msg}"))
}
