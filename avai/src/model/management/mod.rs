mod mutation;
mod service;
mod validation;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base::{tokio::sync::Semaphore, tokio_util::sync::CancellationToken};
use gmv_protocol::common::v1::ErrorDetail;

use crate::{
    model::{ModelError, ModelManager, ModelRepository, ModelResult, PackagePolicy},
    observability::Observability,
    task::TaskManager,
};

#[cfg(test)]
use crate::model::ModelIdentity;
#[cfg(test)]
use mutation::MutationCommand;

const MAX_RECEIPT_CAPACITY: usize = 4_096;

#[derive(Clone)]
pub struct ModelManagementConfig {
    pub trusted_import_root: PathBuf,
    pub package_policy: PackagePolicy,
    pub receipt_capacity: usize,
    pub receipt_retention_ms: i64,
    pub mutation_concurrency: usize,
}

impl ModelManagementConfig {
    pub fn validate(&self) -> ModelResult<()> {
        if self.receipt_capacity == 0 || self.receipt_capacity > MAX_RECEIPT_CAPACITY {
            return Err(ModelError::new(
                "invalid_model_management_config",
                "operation receipt capacity must be between 1 and 4096",
            ));
        }
        if self.receipt_retention_ms < 24 * 60 * 60 * 1_000 {
            return Err(ModelError::new(
                "invalid_model_management_config",
                "operation receipt retention must be at least 24 hours",
            ));
        }
        if self.mutation_concurrency != 1 {
            return Err(ModelError::new(
                "invalid_model_management_config",
                "mutation concurrency must be exactly one",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AvaiModelManagementRpc {
    repository: ModelRepository,
    manager: ModelManager,
    tasks: TaskManager,
    config: Arc<ModelManagementConfig>,
    mutation_lane: Arc<Semaphore>,
    runtime_cancellation: CancellationToken,
    observability: Arc<Observability>,
}

impl AvaiModelManagementRpc {
    pub fn new(
        repository: ModelRepository,
        manager: ModelManager,
        tasks: TaskManager,
        config: ModelManagementConfig,
    ) -> ModelResult<Self> {
        Self::new_with_cancellation(repository, manager, tasks, config, CancellationToken::new())
    }

    pub fn new_with_cancellation(
        repository: ModelRepository,
        manager: ModelManager,
        tasks: TaskManager,
        config: ModelManagementConfig,
        runtime_cancellation: CancellationToken,
    ) -> ModelResult<Self> {
        Self::new_with_observability(
            repository,
            manager,
            tasks,
            config,
            runtime_cancellation,
            Arc::new(Observability::new()),
        )
    }

    pub fn new_with_observability(
        repository: ModelRepository,
        manager: ModelManager,
        tasks: TaskManager,
        config: ModelManagementConfig,
        runtime_cancellation: CancellationToken,
        observability: Arc<Observability>,
    ) -> ModelResult<Self> {
        config.validate()?;
        let mutation_concurrency = config.mutation_concurrency;
        Ok(Self {
            repository,
            manager,
            tasks,
            config: Arc::new(config),
            mutation_lane: Arc::new(Semaphore::new(mutation_concurrency)),
            runtime_cancellation,
            observability,
        })
    }
}

#[cfg(test)]
pub(crate) fn preload_request_hash_for_test(
    identity: ModelIdentity,
    deadline_epoch_ms: i64,
) -> String {
    MutationCommand::Preload(identity).request_hash(deadline_epoch_ms)
}

fn error_detail(code: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: code.to_string(),
        metadata: Default::default(),
    }
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(unix)]
pub async fn serve_uds<O: gmv_nodec::component_management::ComponentDrainOwner>(
    socket: &Path,
    owner: Arc<O>,
    model_rpc: AvaiModelManagementRpc,
    cancel: base::tokio_util::sync::CancellationToken,
) -> base::exception::GlobalResult<()> {
    use crate::feedback::AvaiFeedbackRpc;
    use gmv_nodec::component_management::{ComponentManagementRpc, OwnedUdsListener};
    use gmv_protocol::avai::feedback::v1::avai_feedback_server::AvaiFeedbackServer;
    use gmv_protocol::avai::model_management::v1::avai_model_management_server::AvaiModelManagementServer;
    use gmv_protocol::component_management::v1::component_management_server::ComponentManagementServer;

    let owned = OwnedUdsListener::bind(socket).await?;
    let incoming = owned.incoming();
    let feedback_rpc = AvaiFeedbackRpc::new(model_rpc.tasks.feedback_manager());
    let result = tonic::transport::Server::builder()
        .add_service(AvaiFeedbackServer::new(feedback_rpc))
        .add_service(ComponentManagementServer::new(ComponentManagementRpc::new(
            owner,
        )))
        .add_service(AvaiModelManagementServer::new(model_rpc))
        .serve_with_incoming_shutdown(incoming, async move { cancel.cancelled().await })
        .await;
    owned.cleanup()?;
    result.map_err(|error| base::exception::GlobalError::from_external_error(error, |_| {}))
}
