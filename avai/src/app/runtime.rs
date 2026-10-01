use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use avai::feedback::FeedbackConfig;
use avai::guard_integration::{AvaiControlRpc, AvaiGuardNode};
use avai::model::{
    ModelManager, ModelManagerConfig, ModelRepository, ONNX_CPU_RUNTIME, OnnxCpuConfig,
    OnnxCpuProvider, RuntimeProvider,
};
use avai::model_management::{AvaiModelManagementRpc, ModelManagementConfig, serve_uds};
use avai::observability::Observability;
use avai::source::SourcePolicy;
use avai::task::{AvaiDrainBehavior, TaskManager, TaskManagerConfig};
use avai::upload::{UploadManager, UploadManagerConfig};
use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use gmv_nodec::component_management::ManagedDrainOwner;
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::avai::v1::avai_control_server::AvaiControlServer;

use super::config::{GuardConf, ModelConf, ServerConf, validate_feedback_root};
use super::{App, Bootstrap, config_error, external_error, global_error};

impl Daemon<Bootstrap> for App {
    fn cli_basic() -> CliBasic {
        default_cli_basic!()
    }

    fn init_privilege() -> GlobalResult<(Self, Bootstrap)> {
        base::logger::Logger::init()?;
        let guard = GuardConf::try_conf().map_err(config_error)?;
        let server = ServerConf::try_conf().map_err(config_error)?;
        let model = ModelConf::try_conf().map_err(config_error)?;
        let feedback = FeedbackConfig::try_conf().map_err(config_error)?;
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
                feedback,
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
        mut feedback,
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
    feedback.spool_root =
        validate_feedback_root(&feedback.spool_root, &object_root, Path::new(&model.root))?;
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
    let manager = TaskManager::open_with_feedback(
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
        Some((feedback, node.installation_id.clone(), node.host_id.clone())),
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

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
