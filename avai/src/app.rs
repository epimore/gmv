use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;

use avai::guard_integration::{AvaiControlRpc, AvaiGuardNode};
use avai::source::SourcePolicy;
use avai::task::{TaskManager, TaskManagerConfig};
use avai::upload::{UploadManager, UploadManagerConfig};
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::cfg_lib::{CliBasic, conf, default_cli_basic};
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult};
use base::serde::Deserialize;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::avai::v1::avai_control_server::AvaiControlServer;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard", check)]
struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    endpoint: String,
}

impl CheckFromConf for GuardConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.endpoint.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "guard.endpoint must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server", check)]
struct ServerConf {
    installation_id: String,
    host_id: String,
    #[serde(default = "default_node_id")]
    node_id: String,
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_grpc_port")]
    grpc_port: u16,
    #[serde(default = "default_capabilities")]
    capabilities: Vec<String>,
    #[serde(default = "default_task_database_path")]
    task_database_path: String,
    #[serde(default = "default_object_root")]
    object_root: String,
    #[serde(default = "default_uds_socket_root")]
    uds_socket_root: String,
    #[serde(default = "default_upload_listen_addr")]
    upload_listen_addr: SocketAddr,
    #[serde(default = "default_upload_public_url")]
    upload_public_url: String,
    #[serde(default = "default_max_image_bytes")]
    max_image_bytes: usize,
    #[serde(default = "default_task_queue_size")]
    task_queue_size: usize,
    #[serde(default = "default_task_worker_count")]
    task_worker_count: usize,
    #[serde(default)]
    allow_private_image_urls: bool,
    #[serde(default)]
    allowed_internal_hosts: Vec<String>,
}

impl CheckFromConf for ServerConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.installation_id.trim().is_empty() || self.host_id.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.installation_id and server.host_id must not be empty".to_string(),
            ));
        }
        if self.node_id.trim().is_empty() || self.host.trim().is_empty() || self.grpc_port == 0 {
            return Err(FieldCheckError::BizError(
                "server.node_id, server.host and server.grpc_port are required".to_string(),
            ));
        }
        if self.task_queue_size == 0 || self.task_worker_count == 0 || self.max_image_bytes == 0 {
            return Err(FieldCheckError::BizError(
                "Avai task capacity values must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

pub struct App {
    guard: GuardConf,
    server: ServerConf,
}

pub struct Bootstrap {
    grpc_listener: TcpListener,
    upload_listener: TcpListener,
}

impl Daemon<Bootstrap> for App {
    fn cli_basic() -> CliBasic {
        default_cli_basic!()
    }

    fn init_privilege() -> GlobalResult<(Self, Bootstrap)> {
        base::logger::Logger::init()?;
        let guard = GuardConf::try_conf().map_err(config_error)?;
        let server = ServerConf::try_conf().map_err(config_error)?;
        let grpc_listener =
            TcpListener::bind(("0.0.0.0", server.grpc_port)).map_err(external_error)?;
        grpc_listener
            .set_nonblocking(true)
            .map_err(external_error)?;
        let upload_listener =
            TcpListener::bind(server.upload_listen_addr).map_err(external_error)?;
        upload_listener
            .set_nonblocking(true)
            .map_err(external_error)?;
        Ok((
            Self { guard, server },
            Bootstrap {
                grpc_listener,
                upload_listener,
            },
        ))
    }

    fn run_app(self, bootstrap: Bootstrap) -> GlobalResult<()> {
        let runtime = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        let service_runtime = runtime.clone();
        runtime.spawn("avai-service", async move {
            if let Err(error) = run_service(self, bootstrap, service_runtime).await {
                base::log::error!("avai runtime failed: {error}");
                GlobalRuntime::request_shutdown_with_error();
            }
        })?;
        let report = GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork]);
        if !report.is_graceful() {
            return Err(global_error("avai shutdown was incomplete"));
        }
        Ok(())
    }
}

async fn run_service(app: App, bootstrap: Bootstrap, runtime: GlobalRuntime) -> GlobalResult<()> {
    let App { guard, server } = app;
    let capabilities = server.capabilities.clone();
    let task_database_path = PathBuf::from(&server.task_database_path);
    let object_root = PathBuf::from(&server.object_root);
    let mut node = AvaiGuardNode::new(
        server.node_id,
        generate_instance_id(),
        server.host,
        guard.endpoint,
        u32::from(server.grpc_port),
        capabilities.clone(),
    );
    node.installation_id = server.installation_id;
    node.host_id = server.host_id;
    node.started_at_epoch_ms = now_epoch_ms();
    let manager = TaskManager::open(
        node.identity.clone(),
        capabilities.clone(),
        TaskManagerConfig {
            database_path: task_database_path.clone(),
            queue_size: server.task_queue_size,
            worker_count: server.task_worker_count,
            source_policy: SourcePolicy {
                object_root: object_root.clone(),
                uds_socket_root: server.uds_socket_root.into(),
                max_image_bytes: server.max_image_bytes,
                allow_private_image_urls: server.allow_private_image_urls,
                allowed_internal_hosts: server.allowed_internal_hosts.into_iter().collect(),
                ..SourcePolicy::default()
            },
        },
        &runtime,
    )
    .await
    .map_err(external_error)?;
    let uploads = UploadManager::open(
        node.identity.clone(),
        capabilities.clone(),
        UploadManagerConfig {
            database_path: task_database_path,
            object_root,
            public_url: server.upload_public_url,
            max_image_bytes: server.max_image_bytes,
        },
    )
    .await
    .map_err(external_error)?;
    let snapshot = manager.resource_snapshot().await;
    let rpc = AvaiControlRpc::new_managed(manager.clone(), uploads.clone(), capabilities);
    let metrics_rpc = rpc.clone();
    let mut reporter =
        NodeReporterConfig::new(node.guard_channel.clone(), node.register_request(snapshot));
    reporter.business_metrics = Arc::new(move || {
        HashMap::from([(
            "running_tasks".to_string(),
            metrics_rpc.running_task_count().to_string(),
        )])
    });
    let snapshot_rpc = rpc.clone();
    reporter.resource_snapshot = Some(Arc::new(move || {
        let snapshot_rpc = snapshot_rpc.clone();
        Box::pin(async move { snapshot_rpc.resource_snapshot().await })
    }));
    let cancel = runtime.cancel.clone();
    let event_sender = NodeReporter::spawn_managed_with_events(&runtime, reporter, cancel.clone())?;
    manager.set_event_sender(event_sender).await;

    let upload_listener = base::tokio::net::TcpListener::from_std(bootstrap.upload_listener)
        .map_err(external_error)?;
    let upload_cancel = cancel.clone();
    let upload_app = avai::upload::routes(uploads.clone());
    let upload_task = runtime.spawn("avai-upload-http", async move {
        if let Err(error) = axum::serve(upload_listener, upload_app)
            .with_graceful_shutdown(async move { upload_cancel.cancelled().await })
            .await
        {
            base::log::error!("Avai upload HTTP server failed: {error}");
            GlobalRuntime::request_shutdown_with_error();
        }
    })?;
    let cleanup_cancel = cancel.clone();
    let cleanup_uploads = uploads.clone();
    let upload_cleanup_task = runtime.spawn("avai-upload-cleanup", async move {
        let mut interval = base::tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(base::tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            base::tokio::select! {
                _ = cleanup_cancel.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = cleanup_uploads.cleanup_expired(now_epoch_ms()).await {
                        base::log::warn!(
                            "Avai upload cleanup failed: action=image_upload, stage=cleanup, error_code={}, reason={}",
                            error.code,
                            error.message
                        );
                    }
                }
            }
        }
    })?;
    let address = bootstrap
        .grpc_listener
        .local_addr()
        .map_err(external_error)?;
    let incoming =
        base_rpc::tcp_incoming_from_std(bootstrap.grpc_listener).map_err(external_error)?;
    let shutdown = cancel.clone();
    base::log::debug!(
        "avai rpc service inbound: node_id={}, bind_addr={}",
        node.identity.node_id,
        address
    );
    let serve_result = tonic::transport::Server::builder()
        .add_service(AvaiControlServer::new(rpc))
        .serve_with_incoming_shutdown(incoming, async move { shutdown.cancelled().await })
        .await;
    let expected_shutdown = cancel.is_cancelled();
    if !expected_shutdown {
        if serve_result.is_ok() {
            base::log::error!("avai RPC server stopped unexpectedly");
        }
        GlobalRuntime::request_shutdown_with_error();
    }
    upload_task.await.map_err(external_error)?;
    upload_cleanup_task.await.map_err(external_error)?;
    manager.close_and_wait().await.map_err(external_error)?;
    uploads
        .cleanup_expired(now_epoch_ms())
        .await
        .map_err(external_error)?;
    uploads.close().await;
    match serve_result {
        Ok(()) if expected_shutdown => {
            base::log::debug!("avai rpc service outbound: bind_addr={address}");
            Ok(())
        }
        Ok(()) => Ok(()),
        Err(error) => Err(external_error(error)),
    }
}

fn default_guard_endpoint() -> String {
    "http://127.0.0.1:18080".to_string()
}

fn default_node_id() -> String {
    "avai-node-1".to_string()
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_grpc_port() -> u16 {
    19080
}

fn default_capabilities() -> Vec<String> {
    vec!["image.metadata.inspect".to_string()]
}

fn default_task_database_path() -> String {
    "./data/avai.db".to_string()
}

fn default_object_root() -> String {
    "./data/objects".to_string()
}

fn default_uds_socket_root() -> String {
    "./run".to_string()
}

fn default_upload_listen_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 19081))
}

fn default_upload_public_url() -> String {
    "http://127.0.0.1:19081".to_string()
}

fn default_max_image_bytes() -> usize {
    16 * 1024 * 1024
}

fn default_task_queue_size() -> usize {
    128
}

fn default_task_worker_count() -> usize {
    2
}

fn config_error(error: base::cfg_lib::conf::ConfigError) -> GlobalError {
    GlobalError::from_external_error(error, |_| {})
}

fn external_error<E>(error: E) -> GlobalError
where
    E: std::error::Error + Send + Sync + 'static,
{
    GlobalError::from_external_error(error, |_| {})
}

fn global_error(message: &str) -> GlobalError {
    GlobalError::new_sys_error(message, |_| {})
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
