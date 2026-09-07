use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use avai::guard_integration::{AvaiControlRpc, AvaiGuardNode};
use avai::source::SourcePolicy;
use avai::task::{TaskManager, TaskManagerConfig};
use avai::upload::{UploadManager, UploadManagerConfig};
use base::cfg_lib::conf;
use base::serde::Deserialize;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::avai::v1::avai_control_server::AvaiControlServer;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard")]
struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server")]
struct ServerConf {
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

fn default_guard_endpoint() -> String {
    std::env::var("GMV_GUARD_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:18080".to_string())
}

fn default_node_id() -> String {
    std::env::var("GMV_AVAI_NODE_ID").unwrap_or_else(|_| "avai-node-1".to_string())
}

fn default_host() -> String {
    std::env::var("GMV_AVAI_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn default_grpc_port() -> u16 {
    std::env::var("GMV_AVAI_GRPC_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(19080)
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
    "0.0.0.0:19081"
        .parse()
        .expect("valid upload listen address")
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    base::daemon::install_sanitized_panic_hook();
    base::logger::Logger::init()?;
    let runtime = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
    let service_runtime = runtime.clone();
    runtime.spawn("avai-service", async move {
        if let Err(err) = run_service(service_runtime).await {
            base::log::error!("avai runtime failed: {err}");
            GlobalRuntime::request_shutdown_with_error();
        }
    })?;
    let report = GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork]);
    if !report.is_graceful() {
        return Err(std::io::Error::other("avai shutdown was incomplete").into());
    }
    Ok(())
}

async fn run_service(
    runtime: GlobalRuntime,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let guard = GuardConf::conf();
    let server = ServerConf::conf();
    let capabilities = server.capabilities.clone();
    let task_database_path = std::path::PathBuf::from(&server.task_database_path);
    let object_root = std::path::PathBuf::from(&server.object_root);
    let mut node = AvaiGuardNode::new(
        server.node_id,
        generate_instance_id(),
        server.host,
        guard.endpoint,
        u32::from(server.grpc_port),
        capabilities.clone(),
    );
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
    .map_err(|error| format!("initialize Avai task manager: {}", error.message))?;
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
    .map_err(|error| format!("initialize Avai upload manager: {}", error.message))?;
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
    let upload_listener = base::tokio::net::TcpListener::bind(server.upload_listen_addr).await?;
    let upload_cancel = cancel.clone();
    let upload_app = avai::upload::routes(uploads.clone());
    let upload_task = runtime.spawn("avai-upload-http", async move {
        let result = axum::serve(upload_listener, upload_app)
            .with_graceful_shutdown(async move { upload_cancel.cancelled().await })
            .await;
        if let Err(error) = result {
            base::log::error!("Avai upload HTTP server failed: {error}");
            GlobalRuntime::request_shutdown_with_error();
        }
    })?;
    let address: SocketAddr = format!("0.0.0.0:{}", server.grpc_port).parse()?;
    let shutdown = cancel.clone();
    base::log::debug!(
        "avai rpc service inbound: node_id={}, bind_addr={}",
        node.identity.node_id,
        address
    );
    let serve_result = tonic::transport::Server::builder()
        .add_service(AvaiControlServer::new(rpc))
        .serve_with_shutdown(address, async move { shutdown.cancelled().await })
        .await;
    let expected_shutdown = cancel.is_cancelled();
    if !expected_shutdown {
        if serve_result.is_ok() {
            base::log::error!("avai RPC server stopped unexpectedly");
        }
        GlobalRuntime::request_shutdown_with_error();
    }
    if let Err(error) = upload_task.await {
        return Err(format!("join Avai upload HTTP server: {error}").into());
    }
    manager
        .close_and_wait()
        .await
        .map_err(|error| format!("close Avai task manager: {}", error.message))?;
    uploads.close().await;
    match serve_result {
        Ok(()) if expected_shutdown => {
            base::log::debug!("avai rpc service outbound: bind_addr={address}");
            Ok(())
        }
        Ok(()) => Ok(()),
        Err(err) => Err(err.into()),
    }
}
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
