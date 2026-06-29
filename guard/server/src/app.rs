use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult};
use base::log::warn;
use base::logger;
use base::tokio_util::sync::CancellationToken;

use crate::api::v2::ApiV2;
use crate::app_config::GuardAppConfig;
use crate::auth::Secret;
use crate::core::{GuardError, GuardResult};
use crate::job::SystemJobService;
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
        Ok((Self { config }, GuardListeners { web, rpc }))
    }

    fn run_app(self, listeners: GuardListeners) -> GlobalResult<()> {
        let runtime = base::tokio::runtime::Runtime::new()
            .map_err(|error| GlobalError::new_sys_error(&error.to_string(), |_| {}))?;
        runtime
            .block_on(start_guard(self.config, listeners))
            .map_err(|error| GlobalError::new_sys_error(&error.to_string(), |_| {}))?;
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
    let registry =
        crate::registry::RegistryService::with_policy(store.clone(), config.registry.to_policy());
    let api_store = store.clone();
    let operations = OperationService::default();
    let api = ApiV2::new(store, operations.clone(), SystemJobService::default());
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
        persistent.outbox_repository(),
        users,
        user_repository,
        event_forwarder.clone(),
    );
    let rpc = node_rpc::serve(
        rpc_config,
        listeners.rpc,
        registry,
        api_store.clone(),
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
        PersistentStore::Mysql(store) => CommandIdRepository::from(store.clone()),
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
        loop {
            if worker_cancel.is_cancelled() {
                break;
            }
            if let Err(error) = worker.run_once(now_ms().unwrap_or_default()).await {
                warn!("MQTT outbox worker failed: {error}");
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

fn global_error(message: String) -> GlobalError {
    GlobalError::new_sys_error(&message, |_| {})
}
