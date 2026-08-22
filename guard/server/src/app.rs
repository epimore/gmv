use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use base::logger;
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::utils::rt::{GlobalRuntime, RuntimeType};
use sha2::{Digest, Sha256};

use crate::api::v2::ApiV2;
use crate::app_config::GuardAppConfig;
use crate::auth::{AuthState, SessionPolicy};
use crate::core::{GuardError, GuardResult};
use crate::mqttc::{CommandIdRepository, MqttCommandExecutor, MqttRuntimeManager};
use crate::operation::OperationService;
use crate::outbox::{DeliveryRouter, OutboxWorker};
use crate::runtime::event_forwarder::EventForwarder;
use crate::runtime::node_rpc::{self, NodeRpcConfig};
use crate::runtime::web::{self, WebServerConfig};
use crate::runtime::{lease_expirer, node_expirer};
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
            web_config.tls.is_some(),
            &config.grpc.bind_addr.to_string(),
            config.grpc.tls.enabled,
            |msg| info!("{msg}"),
        );
        Ok((Self { config }, GuardListeners { web, rpc }))
    }

    fn run_app(self, listeners: GuardListeners) -> GlobalResult<()> {
        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        let service_rt = network_rt.clone();
        network_rt.spawn("guard-service", async move {
            if let Err(err) = start_guard(self.config, listeners, service_rt).await {
                error!("Guard runtime failed: {err}");
                GlobalRuntime::request_shutdown_with_error();
            }
        })?;
        let report = GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork]);
        if !report.is_graceful() {
            return Err(GlobalError::new_sys_error(
                "Guard shutdown was incomplete",
                |_| {},
            ));
        }
        Ok(())
    }
}

pub async fn start_guard(
    config: GuardAppConfig,
    listeners: GuardListeners,
    runtime: GlobalRuntime,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let web_config = WebServerConfig::from_app(&config)?;
    let persistent = PersistentStore::connect(&config).await?;
    persistent.initialize(&config).await?;
    let users = persistent.load_users().await?;
    let user_repository = persistent.user_repository();
    let integration_repository = persistent.integration_repository();
    let integration_master_key = integration_repository
        .master_key()
        .await?
        .ok_or_else(|| GuardError::Conflict("integration master key is missing".to_string()))?;
    let integration_secrets = Some(crate::integration::secret::IntegrationSecretManager::new(
        crate::integration::secret::IntegrationSecretCipher::from_base64_key_no_pad(
            &integration_master_key.key_material,
        )?,
    ));
    let store = InMemoryGuardStore::default();
    let auth = AuthState::new(
        users,
        SessionPolicy {
            allowed_origins: web_config.allowed_origins.clone(),
            secure_cookie: web_config.session_cookie_secure,
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
    let mut background_tasks = vec![
        (
            "node-expirer",
            node_expirer::spawn(&runtime, registry.clone(), config.grpc.heartbeat_timeout_ms)?,
        ),
        (
            "lease-expirer",
            lease_expirer::spawn(&runtime, api_store.clone())?,
        ),
    ];
    let command_ids = match &persistent {
        #[cfg(feature = "db-mysql")]
        PersistentStore::Mysql(store) => CommandIdRepository::from(store.clone()),
        #[cfg(feature = "db-sqlite")]
        PersistentStore::Sqlite(store) => CommandIdRepository::from(store.clone()),
    };
    let mqtt_executor = MqttCommandExecutor::new(operations.clone(), api_store.clone())
        .with_auth(auth.clone())
        .with_media_https_http2_verified(config.http.media_https_http2_verified)
        .with_dynamic_result_outbox(
            persistent.outbox_repository(),
            integration_repository.clone(),
        );
    let mqtt_manager = MqttRuntimeManager::new(
        integration_repository.clone(),
        command_ids,
        mqtt_executor,
        integration_secrets.clone(),
    );
    let mqtt_publisher = mqtt_manager.publisher();
    let mqtt_cancel = runtime.cancel.clone();
    let mqtt_shutdown = mqtt_cancel.clone();
    background_tasks.push((
        "mqtt-runtime-manager",
        runtime.spawn("guard-mqtt-runtime-manager", async move {
            if let Err(error) = mqtt_manager.run(mqtt_cancel).await {
                error!(
                    "MQTT runtime manager failed: action=mqtt_runtime, stage=manager, outcome=failed, reason={error}"
                );
                if !mqtt_shutdown.is_cancelled() {
                    GlobalRuntime::request_shutdown_with_error();
                }
            }
        })?,
    ));
    let event_forwarder = Some(
        EventForwarder::new(persistent.outbox_repository(), Vec::new())
            .with_integrations(integration_repository.clone()),
    );
    if let Some(forwarder) = event_forwarder.clone() {
        background_tasks.push((
            "playback-renewal",
            spawn_integration_playback_renewal(&runtime, api_store.clone(), forwarder)?,
        ));
    }
    let mut delivery = DeliveryRouter::default();
    delivery = delivery.with(
        crate::store::model::OutboxDestinationKind::Mqtt,
        Arc::new(mqtt_publisher),
    );
    if let Some(cipher) = integration_secrets.clone() {
        delivery = delivery.with(
            crate::store::model::OutboxDestinationKind::Webhook,
            Arc::new(crate::webhook::IntegrationWebhookDelivery::new(
                integration_repository.clone(),
                cipher,
            )),
        );
    }
    background_tasks.push((
        "outbox-worker",
        spawn_outbox_worker(&runtime, persistent.outbox_repository(), delivery)?,
    ));
    let web = web::serve(
        web_config,
        listeners.web,
        api,
        auth.clone(),
        persistent.outbox_repository(),
        user_repository,
        integration_repository,
        persistent.command_repository(),
        integration_secrets,
        event_forwarder.clone(),
        runtime.cancel.clone(),
    );
    let rpc = node_rpc::serve(
        rpc_config,
        listeners.rpc,
        registry,
        api_store.clone(),
        auth,
        event_forwarder,
        runtime.cancel.clone(),
    );
    let web_cancel = runtime.cancel.clone();
    let web = async {
        let result = web.await;
        if result.is_err() {
            GlobalRuntime::request_shutdown_with_error();
        } else if !web_cancel.is_cancelled() {
            error!("Guard HTTP server stopped unexpectedly");
            GlobalRuntime::request_shutdown_with_error();
        }
        result
    };
    let rpc_cancel = runtime.cancel.clone();
    let rpc = async {
        let result = rpc.await;
        if result.is_err() {
            GlobalRuntime::request_shutdown_with_error();
        } else if !rpc_cancel.is_cancelled() {
            error!("Guard RPC server stopped unexpectedly");
            GlobalRuntime::request_shutdown_with_error();
        }
        result
    };
    let (web_result, rpc_result) = base::tokio::join!(web, rpc);
    for (name, task) in background_tasks {
        match task.await {
            Ok(()) => debug!("Guard background task completed: task={name}"),
            Err(err) if err.is_cancelled() && runtime.cancel.is_cancelled() => {
                debug!("Guard background task cancelled during shutdown: task={name}")
            }
            Err(err) => error!("Guard background task failed: task={name}, reason={err}"),
        }
    }
    persistent.close().await;
    web_result?;
    rpc_result?;
    Ok(())
}

fn spawn_outbox_worker(
    runtime: &GlobalRuntime,
    repository: crate::outbox::OutboxRepository,
    delivery: DeliveryRouter,
) -> GlobalResult<base::tokio::task::JoinHandle<()>> {
    let worker = OutboxWorker::new(
        repository,
        Arc::new(delivery),
        base_rpc::RetryPolicy::default(),
        100,
    )
    .with_delete_delivered(true);
    let cancel = runtime.cancel.clone();
    runtime.spawn("guard-outbox-worker", async move {
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
            base::tokio::select! {
                _ = cancel.cancelled() => break,
                _ = base::tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    })
}

fn spawn_integration_playback_renewal(
    runtime: &GlobalRuntime,
    store: InMemoryGuardStore,
    forwarder: EventForwarder,
) -> GlobalResult<base::tokio::task::JoinHandle<()>> {
    let cancel = runtime.cancel.clone();
    runtime.spawn("guard-playback-renewal", async move {
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
            base::tokio::select! {
                _ = cancel.cancelled() => break,
                _ = base::tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    })
}

fn now_ms() -> GuardResult<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))?
        .as_millis()
        .min(i64::MAX as u128) as i64)
}

fn banner<F: FnOnce(String)>(
    version: &str,
    http_addr: &str,
    http_tls: bool,
    grpc_addr: &str,
    grpc_tls: bool,
    f: F,
) {
    let http_protocol = if http_tls { "HTTPS" } else { "HTTP" };
    let grpc_protocol = if grpc_tls { "gRPC/TLS" } else { "gRPC" };
    let address_width = http_addr.len().max(grpc_addr.len()).max(32);
    let address_border = "─".repeat(address_width + 2);
    let address_header = "Address";
    let banner_width = address_width + 53;
    let separator = "=".repeat(banner_width);
    let title = format!("[GMV:GUARD-SERVER]   Version: {version}");
    let msg = format!(
        r#"
{separator}
{title:^banner_width$}
{separator}
┌──────────────────┬{address_border}┬──────────────┬──────────────┐
│ Service          │ {address_header:<address_width$} │ Protocols    │  Status      │
├──────────────────┼{address_border}┼──────────────┼──────────────┤
│ Guard HTTP       │ {http_addr:<address_width$} │ {http_protocol:<12} │ 🟢 Ready     │
│ Guard RPC        │ {grpc_addr:<address_width$} │ {grpc_protocol:<12} │ 🟢 Listening │
└──────────────────┴{address_border}┴──────────────┴──────────────┘"#
    );
    f(msg);
}

fn global_error(message: String) -> GlobalError {
    GlobalError::new_sys_error(&message, |msg| error!("{msg}"))
}
