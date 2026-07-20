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

use crate::api::v2::ApiV2;
use crate::app_config::GuardAppConfig;
use crate::auth::{AuthState, Secret, SessionPolicy};
use crate::core::{GuardError, GuardResult};
use crate::mqttc::{
    CommandIdRepository, MqttClientConfig, MqttCommandExecutor, MqttCommandPolicy, MqttRuntime,
};
use crate::operation::OperationService;
use crate::outbox::OutboxWorker;
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
    let event_forwarder = if config.integrations.mqtt.enabled {
        spawn_mqtt_runtime(&config, &persistent, operations.clone(), api_store.clone())?
    } else {
        None
    };
    let web = web::serve(
        web_config,
        listeners.web,
        api,
        auth.clone(),
        persistent.outbox_repository(),
        user_repository,
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

fn spawn_mqtt_runtime(
    config: &GuardAppConfig,
    persistent: &PersistentStore,
    operations: OperationService,
    store: InMemoryGuardStore,
) -> GuardResult<Option<EventForwarder>> {
    let mqtt = &config.integrations.mqtt;
    let runtime = MqttRuntime::new(MqttClientConfig {
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
    let event_forwarder = if mqtt.publish_event_topics.is_empty() {
        None
    } else {
        let rules = mqtt
            .publish_event_topics
            .iter()
            .map(|pattern| EventForwardRule {
                pattern: pattern.clone(),
                topic_prefix: mqtt.publish_topic_prefix.clone(),
            })
            .collect::<Vec<_>>();
        Some(EventForwarder::new(persistent.outbox_repository(), rules))
    };
    let topics = mqtt.subscribe_topics.clone();
    let policy = MqttCommandPolicy::new(
        [
            "stream.start".to_string(),
            "stream.stop".to_string(),
            "device.ptz".to_string(),
            "ai.start".to_string(),
            "ai.cancel".to_string(),
        ],
        300_000,
    )?;
    let repository = match persistent {
        #[cfg(feature = "db-mysql")]
        PersistentStore::Mysql(store) => CommandIdRepository::from(store.clone()),
        #[cfg(feature = "db-sqlite")]
        PersistentStore::Sqlite(store) => CommandIdRepository::from(store.clone()),
    };
    let executor = MqttCommandExecutor::new(operations, store);
    let cancel = CancellationToken::new();
    let worker = OutboxWorker::new(
        persistent.outbox_repository(),
        Arc::new(runtime.publisher.clone()),
        runtime.publisher.retry_policy().clone(),
        100,
    )
    .with_max_record_age(Duration::from_secs(mqtt.publish_event_ttl_sec));
    let worker_cancel = cancel.clone();
    base::tokio::spawn(async move {
        let mut failure_episode = FailureEpisode::default();
        loop {
            if worker_cancel.is_cancelled() {
                break;
            }
            match worker.run_once(now_ms().unwrap_or_default()).await {
                Ok(_) => {
                    if let EpisodeDecision::Recovered {
                        total,
                        suppressed,
                        duration,
                    } = failure_episode.record_success(Instant::now())
                    {
                        info!(
                            "MQTT outbox worker state changed: state=ready, previous_state=failed, outcome=recovered, total_failures={total}, suppressed={suppressed}, duration_ms={}",
                            duration.as_millis()
                        );
                    }
                }
                Err(error) => {
                    base::log::trace!("MQTT outbox worker attempt failed: error={error}");
                    match failure_episode.record_failure(Instant::now()) {
                        EpisodeDecision::Started => warn!(
                            "MQTT outbox worker state changed: state=failed, previous_state=ready, reason=run_once_failed"
                        ),
                        EpisodeDecision::Summary {
                            total,
                            since_last_summary,
                            suppressed,
                            duration,
                        } => warn!(
                            "MQTT outbox worker remains failed: outcome=ongoing, reason=run_once_failed, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
                            duration.as_millis()
                        ),
                        EpisodeDecision::Suppressed => {}
                        EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => {
                            unreachable!()
                        }
                    }
                }
            }
            base::tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
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
    Ok(event_forwarder)
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
