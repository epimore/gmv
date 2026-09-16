use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;

use avai::guard_integration::{AvaiControlRpc, AvaiGuardNode};
use avai::model::{
    ExecutionLimits, ModelManager, ModelManagerConfig, ModelRepository, ONNX_CPU_RUNTIME,
    OnnxCpuConfig, OnnxCpuProvider, PackagePolicy, RuntimeProvider,
};
use avai::model_management::{AvaiModelManagementRpc, ModelManagementConfig, serve_uds};
use avai::observability::Observability;
use avai::source::SourcePolicy;
use avai::task::{AvaiDrainBehavior, TaskManager, TaskManagerConfig};
use avai::upload::{UploadManager, UploadManagerConfig};
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::cfg_lib::{CliBasic, conf, default_cli_basic};
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult};
use base::serde::Deserialize;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use gmv_nodec::component_management::ManagedDrainOwner;
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
    #[serde(default = "default_max_result_bytes")]
    max_result_bytes: usize,
    #[serde(default = "default_task_queue_size")]
    task_queue_size: usize,
    #[serde(default = "default_task_worker_count")]
    task_worker_count: usize,
    #[serde(default)]
    allow_private_image_urls: bool,
    #[serde(default)]
    allowed_internal_hosts: Vec<String>,
    #[serde(default)]
    management_socket: Option<PathBuf>,
    #[serde(default = "default_management_component_id")]
    management_component_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
struct ResultSchemaConf {
    name: String,
    version: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "model", check)]
struct ModelConf {
    #[serde(default = "default_model_database_path")]
    database_path: String,
    #[serde(default = "default_model_root")]
    root: String,
    #[serde(default = "default_trusted_import_root")]
    trusted_import_root: String,
    #[serde(default = "default_model_architecture")]
    architecture: String,
    #[serde(default)]
    allowed_runtime_ids: Vec<String>,
    #[serde(default = "default_model_accelerators")]
    allowed_accelerators: Vec<String>,
    #[serde(default)]
    allowed_result_schemas: Vec<ResultSchemaConf>,
    #[serde(default)]
    approved_spdx: Vec<String>,
    #[serde(default)]
    available_license_refs: Vec<String>,
    #[serde(default)]
    trusted_signing_keys: HashMap<String, String>,
    #[serde(default = "default_max_manifest_bytes")]
    max_manifest_bytes: u64,
    #[serde(default = "default_max_model_files")]
    max_file_count: usize,
    #[serde(default = "default_max_package_bytes")]
    max_package_bytes: u64,
    #[serde(default = "default_max_model_memory_mb")]
    max_memory_mb: u64,
    #[serde(default = "default_max_model_vram_mb")]
    max_vram_mb: u64,
    #[serde(default = "default_max_loaded_models")]
    max_loaded_models: usize,
    #[serde(default = "default_native_worker_count")]
    native_worker_count: usize,
    #[serde(default = "default_native_queue_capacity")]
    native_queue_capacity: usize,
    #[serde(default = "default_ort_thread_count")]
    ort_intra_threads: usize,
    #[serde(default = "default_ort_thread_count")]
    ort_inter_threads: usize,
    #[serde(default = "default_native_shutdown_timeout_ms")]
    native_shutdown_timeout_ms: u64,
    #[serde(default = "default_max_input_tensor_elements")]
    max_input_tensor_elements: usize,
    #[serde(default = "default_max_input_tensor_bytes")]
    max_input_tensor_bytes: usize,
    #[serde(default = "default_max_output_tensor_count")]
    max_output_tensor_count: usize,
    #[serde(default = "default_max_output_tensor_elements")]
    max_output_tensor_elements: usize,
    #[serde(default = "default_max_output_tensor_bytes")]
    max_output_tensor_bytes: usize,
    #[serde(default = "default_operation_receipt_capacity")]
    operation_receipt_capacity: usize,
    #[serde(default = "default_operation_retention_ms")]
    operation_retention_ms: i64,
    #[serde(default = "default_mutation_concurrency")]
    mutation_concurrency: usize,
}

impl CheckFromConf for ModelConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.database_path.trim().is_empty()
            || self.root.trim().is_empty()
            || self.trusted_import_root.trim().is_empty()
            || self.architecture.trim().is_empty()
        {
            return Err(FieldCheckError::BizError(
                "model paths and architecture must not be empty".to_string(),
            ));
        }
        if self.max_manifest_bytes == 0
            || self.max_file_count == 0
            || self.max_package_bytes == 0
            || self.max_memory_mb == 0
            || self.max_loaded_models == 0
            || self.native_worker_count == 0
            || self.native_worker_count > 64
            || self.native_queue_capacity == 0
            || self.ort_intra_threads == 0
            || self.ort_inter_threads == 0
            || self.native_shutdown_timeout_ms == 0
            || self.native_shutdown_timeout_ms > 60_000
            || self.max_input_tensor_elements == 0
            || self.max_input_tensor_bytes == 0
            || self.max_output_tensor_count == 0
            || self.max_output_tensor_elements == 0
            || self.max_output_tensor_bytes == 0
            || self.operation_receipt_capacity == 0
            || self.operation_receipt_capacity > 4096
            || self.operation_retention_ms < 24 * 60 * 60 * 1_000
            || self.mutation_concurrency != 1
        {
            return Err(FieldCheckError::BizError(
                "model package, runtime and operation bounds are invalid".to_string(),
            ));
        }
        Ok(())
    }
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
        if self.task_queue_size == 0
            || self.task_worker_count == 0
            || self.max_image_bytes == 0
            || self.max_result_bytes == 0
        {
            return Err(FieldCheckError::BizError(
                "Avai task capacity values must be positive".to_string(),
            ));
        }
        if let Some(socket) = &self.management_socket
            && (!socket.is_absolute()
                || socket.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::CurDir | std::path::Component::ParentDir
                    )
                }))
        {
            return Err(FieldCheckError::BizError(
                "server.management_socket must be an absolute normalized path".to_string(),
            ));
        }
        if self.management_component_id.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.management_component_id must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

pub struct App {
    guard: GuardConf,
    server: ServerConf,
    model: ModelConf,
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
        let model = ModelConf::try_conf().map_err(config_error)?;
        if model.native_worker_count > server.task_worker_count {
            return Err(global_error(
                "model.native_worker_count must not exceed server.task_worker_count",
            ));
        }
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
            Self {
                guard,
                server,
                model,
            },
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
    let App {
        guard,
        server,
        model,
    } = app;
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
    let package_policy = model.package_policy()?;
    let execution_limits = package_policy.execution_limits;
    let model_repository = ModelRepository::open(
        PathBuf::from(&model.database_path).as_path(),
        PathBuf::from(&model.root).as_path(),
    )
    .await
    .map_err(external_error)?;
    let observability = Arc::new(Observability::new());
    match model_repository.count_models().await {
        Ok(count) => observability.set_installed_models(count),
        Err(error) => base::log::warn!(
            "Model telemetry startup reconstruction failed: action=model_lifecycle, stage=startup_restore, outcome=failed, error_code={}",
            error.code
        ),
    }
    let mut providers: Vec<Arc<dyn RuntimeProvider>> = Vec::new();
    let mut onnx_cpu_provider = None;
    if model
        .allowed_runtime_ids
        .iter()
        .any(|runtime| runtime == ONNX_CPU_RUNTIME)
    {
        match OnnxCpuProvider::initialize_from_release(OnnxCpuConfig {
            worker_count: model.native_worker_count,
            queue_capacity: model.native_queue_capacity,
            intra_threads: model.ort_intra_threads,
            inter_threads: model.ort_inter_threads,
            max_result_bytes: server.max_result_bytes,
            shutdown_timeout: std::time::Duration::from_millis(model.native_shutdown_timeout_ms),
            execution_limits,
        }) {
            Ok(provider) => {
                providers.push(Arc::new(provider.clone()));
                onnx_cpu_provider = Some(provider);
            }
            Err(error) => base::log::warn!(
                "Optional ONNX CPU runtime unavailable: action=model_runtime, stage=initialize, runtime=onnx-cpu, error_code={}, error={}",
                error.code,
                error.message
            ),
        }
    }
    let model_manager = ModelManager::open_with_observability(
        model_repository.clone(),
        providers,
        ModelManagerConfig {
            max_loaded_models: model.max_loaded_models,
            max_memory_mb: model.max_memory_mb,
            max_vram_mb: model.max_vram_mb,
        },
        runtime.cancel.clone(),
        observability.clone(),
    )
    .await
    .map_err(external_error)?;
    let manager = TaskManager::open_with_model_manager_and_observability(
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
            max_result_bytes: server.max_result_bytes,
        },
        Some(model_manager.clone()),
        &runtime,
        observability.clone(),
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
    let metrics_observability = observability.clone();
    let mut reporter =
        NodeReporterConfig::new(node.guard_channel.clone(), node.register_request(snapshot));
    reporter.business_metrics = Arc::new(move || {
        let mut metrics = metrics_observability.snapshot();
        metrics.insert(
            "running_tasks".to_string(),
            metrics_rpc.running_task_count().to_string(),
        );
        metrics
    });
    let snapshot_rpc = rpc.clone();
    reporter.resource_snapshot = Some(Arc::new(move || {
        let snapshot_rpc = snapshot_rpc.clone();
        Box::pin(async move { snapshot_rpc.resource_snapshot().await })
    }));
    let cancel = runtime.cancel.clone();
    let event_sender = NodeReporter::spawn_managed_with_events(&runtime, reporter, cancel.clone())?;
    manager.set_event_sender(event_sender).await;

    let management_task = if let Some(socket) = server.management_socket.clone() {
        let owner = Arc::new(ManagedDrainOwner::new(
            server.management_component_id.clone(),
            Arc::new(AvaiDrainBehavior(manager.clone())),
        ));
        let model_rpc = AvaiModelManagementRpc::new_with_observability(
            model_repository.clone(),
            model_manager.clone(),
            manager.clone(),
            ModelManagementConfig {
                trusted_import_root: PathBuf::from(&model.trusted_import_root),
                package_policy,
                receipt_capacity: model.operation_receipt_capacity,
                receipt_retention_ms: model.operation_retention_ms,
                mutation_concurrency: model.mutation_concurrency,
            },
            cancel.clone(),
            observability.clone(),
        )
        .map_err(external_error)?;
        let management_cancel = cancel.clone();
        Some(runtime.spawn("avai-local-management", async move {
            if let Err(error) = serve_uds(&socket, owner, model_rpc, management_cancel).await {
                base::log::error!("Avai local management failed: {error}");
                GlobalRuntime::request_shutdown_with_error();
            }
        })?)
    } else {
        None
    };

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
    manager.mark_runtime_ready();
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
    if let Some(management_task) = management_task {
        management_task.await.map_err(external_error)?;
    }
    if let Some(provider) = onnx_cpu_provider {
        provider.close_and_wait().await.map_err(external_error)?;
    }
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

impl ModelConf {
    fn package_policy(&self) -> GlobalResult<PackagePolicy> {
        use base::base64::Engine;
        let trusted_signing_keys = self
            .trusted_signing_keys
            .iter()
            .map(|(key_id, encoded)| {
                base::base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map(|key| (key_id.clone(), key))
                    .map_err(external_error)
            })
            .collect::<GlobalResult<HashMap<_, _>>>()?;
        Ok(PackagePolicy {
            architecture: self.architecture.clone(),
            available_runtimes: self.allowed_runtime_ids.iter().cloned().collect(),
            available_accelerators: self.allowed_accelerators.iter().cloned().collect(),
            allowed_result_schemas: self
                .allowed_result_schemas
                .iter()
                .map(|schema| (schema.name.clone(), schema.version))
                .collect(),
            approved_spdx: self.approved_spdx.iter().cloned().collect(),
            available_license_refs: self.available_license_refs.iter().cloned().collect(),
            trusted_signing_keys,
            max_manifest_bytes: self.max_manifest_bytes,
            max_file_count: self.max_file_count,
            max_package_bytes: self.max_package_bytes,
            max_memory_mb: self.max_memory_mb,
            max_vram_mb: self.max_vram_mb,
            execution_limits: ExecutionLimits {
                max_input_elements: self.max_input_tensor_elements,
                max_input_bytes: self.max_input_tensor_bytes,
                max_output_tensors: self.max_output_tensor_count,
                max_output_elements: self.max_output_tensor_elements,
                max_output_bytes: self.max_output_tensor_bytes,
            },
        })
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

fn default_max_result_bytes() -> usize {
    1024 * 1024
}

fn default_task_queue_size() -> usize {
    128
}

fn default_task_worker_count() -> usize {
    2
}

fn default_management_component_id() -> String {
    "avai".to_string()
}

fn default_model_database_path() -> String {
    "./data/avai-model.db".to_string()
}
fn default_model_root() -> String {
    "./data/models".to_string()
}
fn default_trusted_import_root() -> String {
    "./data/model-import".to_string()
}
fn default_model_architecture() -> String {
    std::env::consts::ARCH.to_string()
}
fn default_model_accelerators() -> Vec<String> {
    vec!["cpu".to_string()]
}
fn default_max_manifest_bytes() -> u64 {
    256 * 1024
}
fn default_max_model_files() -> usize {
    256
}
fn default_max_package_bytes() -> u64 {
    4 * 1024 * 1024 * 1024
}
fn default_max_model_memory_mb() -> u64 {
    16 * 1024
}
fn default_max_model_vram_mb() -> u64 {
    16 * 1024
}
fn default_max_loaded_models() -> usize {
    8
}
fn default_native_worker_count() -> usize {
    1
}
fn default_native_queue_capacity() -> usize {
    8
}
fn default_ort_thread_count() -> usize {
    1
}
fn default_native_shutdown_timeout_ms() -> u64 {
    5_000
}
fn default_max_input_tensor_elements() -> usize {
    16 * 1024 * 1024
}
fn default_max_input_tensor_bytes() -> usize {
    64 * 1024 * 1024
}
fn default_max_output_tensor_count() -> usize {
    16
}
fn default_max_output_tensor_elements() -> usize {
    16 * 1024 * 1024
}
fn default_max_output_tensor_bytes() -> usize {
    64 * 1024 * 1024
}
fn default_operation_receipt_capacity() -> usize {
    4096
}
fn default_operation_retention_ms() -> i64 {
    24 * 60 * 60 * 1_000
}
fn default_mutation_concurrency() -> usize {
    1
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
