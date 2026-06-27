use std::io::{self, Read};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base::log::warn;
use base::tokio_util::sync::CancellationToken;
use base::utils::crypto::{default_decrypt, default_encrypt};
use guard::api::v2::ApiV2;
use guard::app_config::{GuardAppConfig, config_path_from_args};
use guard::auth::{Role, Secret, hash_password};
use guard::core::{GuardError, GuardResult};
use guard::job::SystemJobService;
use guard::mqttc::{
    CommandIdRepository, MqttClientConfig, MqttCommandExecutor, MqttCommandPolicy, MqttRuntime,
};
use guard::operation::OperationService;
use guard::outbox::OutboxWorker;
use guard::runtime::event_forwarder::{EventForwardRule, EventForwarder};
use guard::runtime::node_expirer;
use guard::runtime::node_rpc::{self, NodeRpcConfig};
use guard::runtime::web::{self, WebServerConfig};
use guard::sim::{EndpointMode, Simulator};
use guard::store::InMemoryGuardStore;
use guard::store::persistent::PersistentStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("reset-admin-password") => reset_admin_password(&args[1..]),
        Some("encrypt") => crypto_command(&args[1..], CryptoAction::Encrypt),
        Some("decrypt") => crypto_command(&args[1..], CryptoAction::Decrypt),
        _ => start_guard(),
    }
}

#[derive(Clone, Copy)]
enum CryptoAction {
    Encrypt,
    Decrypt,
}

fn crypto_command(args: &[String], action: CryptoAction) -> Result<(), Box<dyn std::error::Error>> {
    let label = match action {
        CryptoAction::Encrypt => "plaintext",
        CryptoAction::Decrypt => "ciphertext",
    };
    let input = match args {
        [value] if !value.is_empty() => value,
        [_] => {
            return Err(GuardError::InvalidConfig(format!("{label} is required")).into());
        }
        _ => {
            return Err(GuardError::InvalidConfig(
                "usage: guard encrypt|decrypt <value>".to_string(),
            )
            .into());
        }
    };
    let output = match action {
        CryptoAction::Encrypt => default_encrypt(&input),
        CryptoAction::Decrypt => default_decrypt(&input),
    }
    .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
    println!("{output}");
    Ok(())
}

fn start_guard() -> Result<(), Box<dyn std::error::Error>> {
    let config = GuardAppConfig::load(config_path_from_args()?);
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let web_config = WebServerConfig::from_app(&config)?;
        let persistent = PersistentStore::connect(&config).await?;
        persistent.initialize(&config).await?;
        let users = persistent.load_users().await?;
        let user_repository = persistent.user_repository();
        let store = InMemoryGuardStore::default();
        let registry = guard::registry::RegistryService::with_policy(
            store.clone(),
            config.registry.to_policy(),
        );
        let simulator = if web_config.simulator_enabled {
            let simulator = Simulator::new(store.clone(), EndpointMode::Single);
            simulator.bootstrap(0)?;
            Some(simulator)
        } else {
            None
        };
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
            api,
            persistent.outbox_repository(),
            simulator,
            users,
            user_repository,
            persistent.media_repository(),
            event_forwarder.clone(),
        );
        let rpc = node_rpc::serve(
            rpc_config,
            registry,
            api_store.clone(),
            event_forwarder,
            persistent.media_repository(),
        );
        base::tokio::try_join!(web, rpc).map(|_| ())
    })
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

fn reset_admin_password(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (config_path, username) = reset_admin_password_args(args)?;
    let config = GuardAppConfig::load(config_path);
    let username = username.unwrap_or_else(|| config.bootstrap.admin.username.clone());
    let password = read_required_stdin("password")?;
    let password_hash = hash_password(&password)?;
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let persistent = PersistentStore::connect(&config).await?;
        if config.database.auto_migrate {
            persistent.migrate().await?;
        }
        let users = persistent.user_repository();
        let existing = users
            .load_user(&username)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("user {username}")))?;
        users
            .upsert_user(
                &username,
                Role::Admin,
                Some(&password_hash),
                Some(&existing.nickname),
                true,
                now_ms()?,
            )
            .await?;
        users.revoke_ui_sessions(&username).await?;
        Ok::<_, GuardError>(())
    })?;
    println!("reset admin password for user {username}");
    Ok(())
}

fn reset_admin_password_args(args: &[String]) -> GuardResult<(String, Option<String>)> {
    let mut config_path = "./config.yml".to_string();
    let mut username = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-c" | "--config" => {
                index += 1;
                config_path = args.get(index).cloned().ok_or_else(reset_usage)?;
            }
            "-u" | "--username" => {
                index += 1;
                let value = args.get(index).cloned().ok_or_else(reset_usage)?;
                if value.trim().is_empty() {
                    return Err(reset_usage());
                }
                username = Some(value);
            }
            _ => return Err(reset_usage()),
        }
        index += 1;
    }
    Ok((config_path, username))
}

fn reset_usage() -> GuardError {
    GuardError::InvalidConfig(
        "usage: guard reset-admin-password [-c|--config <path>] [-u|--username <name>]".to_string(),
    )
}

fn read_required_stdin(label: &str) -> GuardResult<String> {
    let mut value = String::new();
    io::stdin()
        .read_to_string(&mut value)
        .map_err(|error| GuardError::InvalidConfig(format!("read {label} failed: {error}")))?;
    let value = value.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        return Err(GuardError::InvalidConfig(format!("{label} is required")));
    }
    Ok(value)
}

fn now_ms() -> GuardResult<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))?
        .as_millis()
        .min(i64::MAX as u128) as i64)
}
