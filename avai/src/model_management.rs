use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base::{
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    sha2::{Digest, Sha256},
    tokio::sync::Semaphore,
    tokio_util::sync::CancellationToken,
};
use gmv_protocol::{
    avai::model_management::v1::{
        ActivateModelRequest, ImportStagedModelRequest, InspectModelRequest, InspectModelResponse,
        ListModelsRequest, ListModelsResponse, ModelHealth, ModelIdentity as RpcModelIdentity,
        ModelLifecycleState, ModelMutationResponse, ModelOperationOutcome, ModelSnapshot,
        PreloadModelRequest, RollbackModelRequest, UnloadModelRequest,
        avai_model_management_server::AvaiModelManagement,
    },
    common::v1::{ErrorDetail, OperationRef},
};
use tonic::{Request, Response, Status};

use crate::{
    model::{
        ClaimOperation, InstalledModel, ModelError, ModelIdentity, ModelManager, ModelObservation,
        ModelRepository, ModelResult, ModelState, OperationClaimRequest, OperationReceipt,
        OperationReceiptLimits, OperationReceiptState, PackagePolicy, RuntimeCallContext,
        verify_package,
    },
    observability::Observability,
    task::TaskManager,
};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 200;
const MAX_ID_BYTES: usize = 128;
const MAX_DEADLINE_AHEAD_MS: i64 = 2 * 60 * 60 * 1_000;
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

    async fn snapshot(
        &self,
        model: InstalledModel,
        observe_live_health: bool,
        runtime_context: Option<RuntimeCallContext>,
    ) -> ModelSnapshot {
        let observed_at_epoch_ms = now_epoch_ms();
        let observation = self.manager.observation(&model.identity).await;
        let runtime_available = self.manager.runtime_available(&model.runtime);
        let (health, error) = if !runtime_available {
            (
                ModelHealth::Unavailable,
                Some(error_detail("model_runtime_unavailable")),
            )
        } else if !observe_live_health || !observation.loaded {
            (ModelHealth::Unknown, None)
        } else {
            let context = runtime_context.expect("live health has a validated runtime context");
            match self
                .manager
                .health_with_context(&model.identity, context)
                .await
            {
                Ok(()) => (ModelHealth::Healthy, None),
                Err(error)
                    if matches!(
                        error.code,
                        "model_runtime_deadline_exceeded" | "model_runtime_cancelled"
                    ) =>
                {
                    (
                        ModelHealth::Unknown,
                        Some(error_detail("model_deadline_exceeded")),
                    )
                }
                Err(_) => (
                    ModelHealth::Unhealthy,
                    Some(error_detail("model_health_failed")),
                ),
            }
        };
        model_snapshot(
            model,
            observation,
            runtime_available,
            health,
            error,
            observed_at_epoch_ms,
        )
    }

    async fn execute_mutation(
        &self,
        operation: Option<OperationRef>,
        deadline_epoch_ms: i64,
        command: MutationCommand,
    ) -> ModelMutationResponse {
        let now = now_epoch_ms();
        let operation = match validate_operation_identity(operation) {
            Ok(operation) => operation,
            Err(error) => return mutation_failure("", error.code, false, now, None),
        };
        let request_hash = command.request_hash(deadline_epoch_ms);
        let claim_request = || OperationClaimRequest {
            operation_id: &operation.operation_id,
            idempotency_key: &operation.idempotency_key,
            operation_kind: command.kind(),
            request_hash: &request_hash,
            deadline_epoch_ms,
            now_epoch_ms: now,
        };
        let existing = match self.repository.find_operation(&claim_request()).await {
            Ok(Some(receipt)) if receipt.state != OperationReceiptState::Pending => {
                return self
                    .replay_terminal(receipt, command.observed_identity())
                    .await;
            }
            Ok(receipt) => receipt,
            Err(error) => {
                return mutation_failure(
                    &operation.operation_id,
                    error.code,
                    false,
                    now_epoch_ms(),
                    None,
                );
            }
        };
        if existing.is_none()
            && let Err(error) = validate_new_deadline(deadline_epoch_ms, now)
        {
            return mutation_failure(&operation.operation_id, error.code, false, now, None);
        }
        let permit = match self.mutation_lane.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return mutation_failure(
                    &operation.operation_id,
                    "model_operation_busy",
                    false,
                    now,
                    None,
                );
            }
        };
        let (receipt, resumed) = match self.repository.find_operation(&claim_request()).await {
            Ok(Some(receipt)) if receipt.state != OperationReceiptState::Pending => {
                drop(permit);
                return self
                    .replay_terminal(receipt, command.observed_identity())
                    .await;
            }
            Ok(Some(receipt)) => (receipt, true),
            Ok(None) => {
                if let Err(error) = validate_new_deadline(deadline_epoch_ms, now_epoch_ms()) {
                    drop(permit);
                    return mutation_failure(
                        &operation.operation_id,
                        error.code,
                        false,
                        now_epoch_ms(),
                        None,
                    );
                }
                match self
                    .repository
                    .claim_operation(
                        claim_request(),
                        OperationReceiptLimits {
                            retention_ms: self.config.receipt_retention_ms,
                            capacity: self.config.receipt_capacity,
                        },
                    )
                    .await
                {
                    Ok(ClaimOperation::New(receipt)) => (receipt, false),
                    Ok(ClaimOperation::Existing(receipt))
                        if receipt.state != OperationReceiptState::Pending =>
                    {
                        drop(permit);
                        return self
                            .replay_terminal(receipt, command.observed_identity())
                            .await;
                    }
                    Ok(ClaimOperation::Existing(receipt)) => (receipt, true),
                    Err(error) => {
                        drop(permit);
                        return mutation_failure(
                            &operation.operation_id,
                            error.code,
                            false,
                            now_epoch_ms(),
                            None,
                        );
                    }
                }
            }
            Err(error) => {
                drop(permit);
                return mutation_failure(
                    &operation.operation_id,
                    error.code,
                    false,
                    now_epoch_ms(),
                    None,
                );
            }
        };
        let service = self.clone();
        let operation_id = operation.operation_id.clone();
        let observed_identity = command.observed_identity().cloned();
        let operation_kind = command.kind();
        let refresh_installed = matches!(command, MutationCommand::Import { .. });
        let (sender, receiver) = base::tokio::sync::oneshot::channel();
        let runtime_context =
            runtime_context_from_epoch(deadline_epoch_ms, self.runtime_cancellation.clone());
        base::tokio::spawn(async move {
            let _permit = permit;
            let reconciliation = if resumed {
                Some(command.is_committed(&service).await)
            } else {
                None
            };
            let result = match reconciliation {
                Some(Ok(true)) => Ok(()),
                Some(Err(error)) => Err(error),
                Some(Ok(false)) | None if deadline_epoch_ms <= now_epoch_ms() => {
                    Err(ModelError::new(
                        "model_deadline_exceeded",
                        "model operation deadline has expired",
                    ))
                }
                Some(Ok(false)) | None => {
                    command
                        .execute(&service, deadline_epoch_ms, runtime_context)
                        .await
                }
            };
            if result.is_ok() && refresh_installed {
                match service.repository.count_models().await {
                    Ok(count) => service.observability.set_installed_models(count),
                    Err(error) => base::log::warn!(
                        "Model telemetry refresh failed: action=model_lifecycle, stage=install, outcome=failed, error_code={}",
                        error.code
                    ),
                }
            }
            let terminal_at = now_epoch_ms();
            let (state, stable_error_code) = match &result {
                Ok(()) => (OperationReceiptState::Succeeded, None),
                Err(error) => (OperationReceiptState::Failed, Some(error.code)),
            };
            let finish = service
                .repository
                .finish_operation(&operation_id, state, stable_error_code, terminal_at)
                .await;
            let response = match finish {
                Err(_) => mutation_failure(
                    &operation_id,
                    "model_operation_receipt_failed",
                    false,
                    terminal_at,
                    None,
                ),
                Ok(()) => {
                    match &result {
                        Ok(()) => base::log::info!(
                            "Model operation completed: action=model_lifecycle, stage={}, outcome=succeeded, operation_id={}",
                            operation_kind.to_ascii_lowercase(),
                            operation_id
                        ),
                        Err(error) => base::log::warn!(
                            "Model operation failed: action=model_lifecycle, stage={}, outcome=failed, operation_id={}, error_code={}",
                            operation_kind.to_ascii_lowercase(),
                            operation_id,
                            error.code
                        ),
                    }
                    let snapshot = service.snapshot_optional(observed_identity.as_ref()).await;
                    match result {
                        Ok(()) => mutation_success(&operation_id, resumed, terminal_at, snapshot),
                        Err(error) => mutation_failure(
                            &operation_id,
                            error.code,
                            resumed,
                            terminal_at,
                            snapshot,
                        ),
                    }
                }
            };
            let _ = sender.send(response);
        });
        receiver.await.unwrap_or_else(|_| {
            mutation_failure(
                &receipt.operation_id,
                "model_operation_owner_lost",
                false,
                now_epoch_ms(),
                None,
            )
        })
    }

    async fn snapshot_optional(&self, identity: Option<&ModelIdentity>) -> Option<ModelSnapshot> {
        let identity = identity?;
        let model = self.repository.get(identity).await.ok().flatten()?;
        Some(self.snapshot(model, false, None).await)
    }

    async fn replay_terminal(
        &self,
        receipt: OperationReceipt,
        identity: Option<&ModelIdentity>,
    ) -> ModelMutationResponse {
        let snapshot = self.snapshot_optional(identity).await;
        match receipt.state {
            OperationReceiptState::Succeeded => {
                mutation_success(&receipt.operation_id, true, now_epoch_ms(), snapshot)
            }
            OperationReceiptState::Failed => mutation_failure(
                &receipt.operation_id,
                receipt
                    .stable_error_code
                    .as_deref()
                    .unwrap_or("model_operation_failed"),
                true,
                now_epoch_ms(),
                snapshot,
            ),
            OperationReceiptState::Pending => unreachable!("pending receipt is not terminal"),
        }
    }
}

#[tonic::async_trait]
impl AvaiModelManagement for AvaiModelManagementRpc {
    async fn list_models(
        &self,
        request: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let request = request.into_inner();
        let limit = if request.page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            usize::try_from(request.page_size).unwrap_or(MAX_PAGE_SIZE + 1)
        };
        if limit > MAX_PAGE_SIZE {
            return Ok(Response::new(list_failure("model_page_size_invalid")));
        }
        let after = match decode_page_token(&request.page_token) {
            Ok(after) => after,
            Err(error) => return Ok(Response::new(list_failure(error.code))),
        };
        let mut models = match self.repository.list_page(after.as_ref(), limit + 1).await {
            Ok(models) => models,
            Err(error) => return Ok(Response::new(list_failure(error.code))),
        };
        let next_page_token = if models.len() > limit {
            let next = models.pop().expect("page overflow has a continuation row");
            models
                .last()
                .map(|model| encode_page_token(&model.identity))
                .unwrap_or_else(|| encode_page_token(&next.identity))
        } else {
            String::new()
        };
        let mut snapshots = Vec::with_capacity(models.len());
        for model in models {
            snapshots.push(self.snapshot(model, false, None).await);
        }
        Ok(Response::new(ListModelsResponse {
            models: snapshots,
            next_page_token,
            error: None,
            observed_at_epoch_ms: now_epoch_ms(),
        }))
    }

    async fn inspect_model(
        &self,
        request: Request<InspectModelRequest>,
    ) -> Result<Response<InspectModelResponse>, Status> {
        let request = request.into_inner();
        let identity = match rpc_identity(request.identity) {
            Ok(identity) => identity,
            Err(error) => return Ok(Response::new(inspect_failure(error.code))),
        };
        let model = match self.repository.get(&identity).await {
            Ok(Some(model)) => model,
            Ok(None) => return Ok(Response::new(inspect_failure("model_not_found"))),
            Err(error) => return Ok(Response::new(inspect_failure(error.code))),
        };
        if request.observe_live_health
            && (request.deadline_epoch_ms <= now_epoch_ms()
                || request.deadline_epoch_ms > now_epoch_ms().saturating_add(MAX_DEADLINE_AHEAD_MS))
        {
            return Ok(Response::new(inspect_failure("model_deadline_invalid")));
        }
        let snapshot = self
            .snapshot(
                model,
                request.observe_live_health,
                request.observe_live_health.then(|| {
                    runtime_context_from_epoch(
                        request.deadline_epoch_ms,
                        self.runtime_cancellation.clone(),
                    )
                }),
            )
            .await;
        Ok(Response::new(InspectModelResponse {
            model: Some(snapshot),
            error: None,
            observed_at_epoch_ms: now_epoch_ms(),
        }))
    }

    async fn import_staged_model(
        &self,
        request: Request<ImportStagedModelRequest>,
    ) -> Result<Response<ModelMutationResponse>, Status> {
        let request = request.into_inner();
        let identity = match rpc_identity(request.expected_identity) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(Response::new(mutation_failure(
                    "",
                    error.code,
                    false,
                    now_epoch_ms(),
                    None,
                )));
            }
        };
        Ok(Response::new(
            self.execute_mutation(
                request.operation,
                request.deadline_epoch_ms,
                MutationCommand::Import {
                    stage_id: request.stage_id,
                    identity,
                    manifest_sha256: request.expected_manifest_sha256,
                },
            )
            .await,
        ))
    }

    async fn preload_model(
        &self,
        request: Request<PreloadModelRequest>,
    ) -> Result<Response<ModelMutationResponse>, Status> {
        let request = request.into_inner();
        Ok(Response::new(
            mutation_for_identity(
                self,
                request.operation,
                request.deadline_epoch_ms,
                request.identity,
                MutationKind::Preload,
            )
            .await,
        ))
    }

    async fn activate_model(
        &self,
        request: Request<ActivateModelRequest>,
    ) -> Result<Response<ModelMutationResponse>, Status> {
        let request = request.into_inner();
        Ok(Response::new(
            mutation_for_identity(
                self,
                request.operation,
                request.deadline_epoch_ms,
                request.identity,
                MutationKind::Activate,
            )
            .await,
        ))
    }

    async fn rollback_model(
        &self,
        request: Request<RollbackModelRequest>,
    ) -> Result<Response<ModelMutationResponse>, Status> {
        let request = request.into_inner();
        let from = match rpc_identity(request.from_identity) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(Response::new(mutation_failure(
                    "",
                    error.code,
                    false,
                    now_epoch_ms(),
                    None,
                )));
            }
        };
        let to = match rpc_identity(request.to_identity) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(Response::new(mutation_failure(
                    "",
                    error.code,
                    false,
                    now_epoch_ms(),
                    None,
                )));
            }
        };
        Ok(Response::new(
            self.execute_mutation(
                request.operation,
                request.deadline_epoch_ms,
                MutationCommand::Rollback { from, to },
            )
            .await,
        ))
    }

    async fn unload_model(
        &self,
        request: Request<UnloadModelRequest>,
    ) -> Result<Response<ModelMutationResponse>, Status> {
        let request = request.into_inner();
        Ok(Response::new(
            mutation_for_identity(
                self,
                request.operation,
                request.deadline_epoch_ms,
                request.identity,
                MutationKind::Unload,
            )
            .await,
        ))
    }
}

#[derive(Clone, Copy)]
enum MutationKind {
    Preload,
    Activate,
    Unload,
}

async fn mutation_for_identity(
    service: &AvaiModelManagementRpc,
    operation: Option<OperationRef>,
    deadline_epoch_ms: i64,
    identity: Option<RpcModelIdentity>,
    kind: MutationKind,
) -> ModelMutationResponse {
    let identity = match rpc_identity(identity) {
        Ok(identity) => identity,
        Err(error) => return mutation_failure("", error.code, false, now_epoch_ms(), None),
    };
    let command = match kind {
        MutationKind::Preload => MutationCommand::Preload(identity),
        MutationKind::Activate => MutationCommand::Activate(identity),
        MutationKind::Unload => MutationCommand::Unload(identity),
    };
    service
        .execute_mutation(operation, deadline_epoch_ms, command)
        .await
}

enum MutationCommand {
    Import {
        stage_id: String,
        identity: ModelIdentity,
        manifest_sha256: String,
    },
    Preload(ModelIdentity),
    Activate(ModelIdentity),
    Rollback {
        from: ModelIdentity,
        to: ModelIdentity,
    },
    Unload(ModelIdentity),
}

impl MutationCommand {
    fn kind(&self) -> &'static str {
        match self {
            Self::Import { .. } => "IMPORT",
            Self::Preload(_) => "PRELOAD",
            Self::Activate(_) => "ACTIVATE",
            Self::Rollback { .. } => "ROLLBACK",
            Self::Unload(_) => "UNLOAD",
        }
    }

    fn observed_identity(&self) -> Option<&ModelIdentity> {
        match self {
            Self::Import { identity, .. }
            | Self::Preload(identity)
            | Self::Activate(identity)
            | Self::Unload(identity) => Some(identity),
            Self::Rollback { to, .. } => Some(to),
        }
    }

    fn request_hash(&self, deadline_epoch_ms: i64) -> String {
        let mut hash = Sha256::new();
        hash_part(&mut hash, self.kind().as_bytes());
        hash_part(&mut hash, &deadline_epoch_ms.to_be_bytes());
        match self {
            Self::Import {
                stage_id,
                identity,
                manifest_sha256,
            } => {
                hash_part(&mut hash, stage_id.as_bytes());
                hash_identity(&mut hash, identity);
                hash_part(&mut hash, manifest_sha256.as_bytes());
            }
            Self::Preload(identity) | Self::Activate(identity) | Self::Unload(identity) => {
                hash_identity(&mut hash, identity)
            }
            Self::Rollback { from, to } => {
                hash_identity(&mut hash, from);
                hash_identity(&mut hash, to);
            }
        }
        format!("{:x}", hash.finalize())
    }

    async fn is_committed(&self, service: &AvaiModelManagementRpc) -> ModelResult<bool> {
        match self {
            Self::Import {
                identity,
                manifest_sha256,
                ..
            } => match service.repository.get(identity).await? {
                Some(model) if model.manifest_sha256.eq_ignore_ascii_case(manifest_sha256) => {
                    Ok(true)
                }
                Some(_) => Err(ModelError::new(
                    "model_revision_conflict",
                    "immutable model revision has different content",
                )),
                None => Ok(false),
            },
            Self::Preload(identity) => Ok(service.manager.observation(identity).await.loaded),
            Self::Activate(identity) => {
                let model = required_model(&service.repository, identity).await?;
                Ok(same_capabilities(
                    &service
                        .manager
                        .observation(identity)
                        .await
                        .active_capabilities,
                    &model.capabilities,
                ))
            }
            Self::Rollback { from, to } => {
                let from_model = required_model(&service.repository, from).await?;
                let to_model = required_model(&service.repository, to).await?;
                let from_observation = service.manager.observation(from).await;
                let to_observation = service.manager.observation(to).await;
                Ok(
                    same_capabilities(&to_observation.active_capabilities, &to_model.capabilities)
                        && same_capabilities(
                            &from_observation.previous_capabilities,
                            &from_model.capabilities,
                        ),
                )
            }
            Self::Unload(identity) => {
                let model = required_model(&service.repository, identity).await?;
                let observation = service.manager.observation(identity).await;
                Ok(!observation.loaded
                    && observation.active_capabilities.is_empty()
                    && observation.previous_capabilities.is_empty()
                    && model.state == ModelState::Installed)
            }
        }
    }

    async fn execute(
        self,
        service: &AvaiModelManagementRpc,
        deadline_epoch_ms: i64,
        runtime_context: RuntimeCallContext,
    ) -> ModelResult<()> {
        match self {
            Self::Import {
                stage_id,
                identity,
                manifest_sha256,
            } => {
                if !valid_stage_id(&stage_id) || !valid_sha256(&manifest_sha256) {
                    return Err(ModelError::new(
                        "model_stage_invalid",
                        "stage identity or manifest hash is invalid",
                    ));
                }
                if let Some(existing) = service.repository.get(&identity).await? {
                    return if existing
                        .manifest_sha256
                        .eq_ignore_ascii_case(&manifest_sha256)
                    {
                        Ok(())
                    } else {
                        Err(ModelError::new(
                            "model_revision_conflict",
                            "immutable model revision has different content",
                        ))
                    };
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                let candidate =
                    trusted_stage_candidate(&service.config.trusted_import_root, &stage_id)?;
                let policy = service.config.package_policy.clone();
                let package =
                    base::tokio::task::spawn_blocking(move || verify_package(&candidate, &policy))
                        .await
                        .map_err(|error| {
                            ModelError::new("model_verify_failed", error.to_string())
                        })??;
                if package.manifest.metadata != identity
                    || !package
                        .manifest_sha256
                        .eq_ignore_ascii_case(&manifest_sha256)
                {
                    return Err(ModelError::new(
                        "model_stage_conflict",
                        "verified package does not match expected immutable identity",
                    ));
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                let repository = service.repository.clone();
                let runtime = base::tokio::runtime::Handle::current();
                base::tokio::task::spawn_blocking(move || {
                    runtime.block_on(repository.install(&package, now_epoch_ms()))
                })
                .await
                .map_err(|error| ModelError::new("model_install_failed", error.to_string()))??;
                Ok(())
            }
            Self::Preload(identity) => {
                let model = required_model(&service.repository, &identity).await?;
                if service.manager.observation(&identity).await.loaded {
                    return Ok(());
                }
                if !service.manager.runtime_available(&model.runtime) {
                    return Err(runtime_unavailable());
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                service
                    .manager
                    .preload_with_context(&identity, now_epoch_ms(), runtime_context)
                    .await
            }
            Self::Activate(identity) => {
                let model = required_model(&service.repository, &identity).await?;
                let observation = service.manager.observation(&identity).await;
                if same_capabilities(&observation.active_capabilities, &model.capabilities) {
                    return Ok(());
                }
                if !observation.active_capabilities.is_empty() {
                    return Err(ModelError::new(
                        "model_slot_conflict",
                        "model owns only part of its declared capability set",
                    ));
                }
                if !service.manager.runtime_available(&model.runtime) {
                    return Err(runtime_unavailable());
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                service
                    .manager
                    .activate_with_context(&identity, now_epoch_ms(), runtime_context)
                    .await
                    .map(|_| ())
            }
            Self::Rollback { from, to } => {
                let from_model = required_model(&service.repository, &from).await?;
                let to_model = required_model(&service.repository, &to).await?;
                let to_observation = service.manager.observation(&to).await;
                let from_observation = service.manager.observation(&from).await;
                if same_capabilities(&to_observation.active_capabilities, &to_model.capabilities)
                    && same_capabilities(
                        &from_observation.previous_capabilities,
                        &from_model.capabilities,
                    )
                {
                    return Ok(());
                }
                if !service.manager.runtime_available(&to_model.runtime) {
                    return Err(runtime_unavailable());
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                service
                    .manager
                    .rollback_exact_with_context(&from, &to, now_epoch_ms(), runtime_context)
                    .await
                    .map(|_| ())
            }
            Self::Unload(identity) => {
                let model = required_model(&service.repository, &identity).await?;
                let observation = service.manager.observation(&identity).await;
                if !observation.loaded && model.state == ModelState::Installed {
                    return Ok(());
                }
                if service
                    .tasks
                    .durable_nonterminal_task_count()
                    .await
                    .map_err(|error| ModelError::new("model_task_guard_failed", error.message))?
                    != 0
                {
                    return Err(ModelError::new(
                        "model_in_use",
                        "durable nonterminal tasks conservatively block unload",
                    ));
                }
                ensure_before_deadline(deadline_epoch_ms)?;
                service.manager.unload(&identity, now_epoch_ms()).await
            }
        }
    }
}

fn runtime_context_from_epoch(
    deadline_epoch_ms: i64,
    cancellation: CancellationToken,
) -> RuntimeCallContext {
    let remaining = deadline_epoch_ms.saturating_sub(now_epoch_ms()).max(0);
    RuntimeCallContext {
        deadline: Instant::now()
            + Duration::from_millis(u64::try_from(remaining).unwrap_or_default()),
        cancellation,
    }
}

#[cfg(test)]
pub(crate) fn preload_request_hash_for_test(
    identity: ModelIdentity,
    deadline_epoch_ms: i64,
) -> String {
    MutationCommand::Preload(identity).request_hash(deadline_epoch_ms)
}

async fn required_model(
    repository: &ModelRepository,
    identity: &ModelIdentity,
) -> ModelResult<InstalledModel> {
    repository
        .get(identity)
        .await?
        .ok_or_else(|| ModelError::new("model_not_found", "installed model does not exist"))
}

fn same_capabilities(left: &[String], right: &[String]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

fn validate_operation_identity(operation: Option<OperationRef>) -> ModelResult<OperationRef> {
    let operation = operation.ok_or_else(|| {
        ModelError::new("model_operation_invalid", "operation identity is required")
    })?;
    if !valid_bounded_token(&operation.operation_id)
        || !valid_bounded_token(&operation.idempotency_key)
    {
        return Err(ModelError::new(
            "model_operation_invalid",
            "operation identifiers are invalid",
        ));
    }
    Ok(operation)
}

fn validate_new_deadline(deadline: i64, now: i64) -> ModelResult<()> {
    if deadline <= now || deadline > now.saturating_add(MAX_DEADLINE_AHEAD_MS) {
        return Err(ModelError::new(
            "model_deadline_invalid",
            "operation deadline is outside the allowed window",
        ));
    }
    Ok(())
}

fn rpc_identity(identity: Option<RpcModelIdentity>) -> ModelResult<ModelIdentity> {
    let identity = identity
        .ok_or_else(|| ModelError::new("model_identity_invalid", "model identity is required"))?;
    if [&identity.model_id, &identity.version, &identity.revision]
        .iter()
        .any(|part| !valid_bounded_token(part))
    {
        return Err(ModelError::new(
            "model_identity_invalid",
            "model identity is invalid",
        ));
    }
    Ok(ModelIdentity {
        model_id: identity.model_id,
        version: identity.version,
        revision: identity.revision,
    })
}

fn valid_bounded_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_stage_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn ensure_before_deadline(deadline: i64) -> ModelResult<()> {
    if now_epoch_ms() >= deadline {
        Err(ModelError::new(
            "model_deadline_exceeded",
            "model operation deadline expired",
        ))
    } else {
        Ok(())
    }
}

fn runtime_unavailable() -> ModelError {
    ModelError::new(
        "model_runtime_unavailable",
        "runtime provider is unavailable",
    )
}

#[cfg(unix)]
fn trusted_stage_candidate(root: &Path, stage_id: &str) -> ModelResult<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root_meta = std::fs::symlink_metadata(root)
        .map_err(|error| ModelError::io("inspect trusted import root", error))?;
    if !root_meta.is_dir()
        || root_meta.file_type().is_symlink()
        || root_meta.permissions().mode() & 0o022 != 0
    {
        return Err(ModelError::new(
            "model_stage_insecure",
            "trusted import root must be a non-writable real directory",
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ModelError::io("resolve trusted import root", error))?;
    let candidate = root.join(stage_id);
    let candidate_meta = std::fs::symlink_metadata(&candidate)
        .map_err(|error| ModelError::io("inspect staged model", error))?;
    if !candidate_meta.is_dir()
        || candidate_meta.file_type().is_symlink()
        || candidate_meta.permissions().mode() & 0o022 != 0
        || candidate_meta.uid() != root_meta.uid()
    {
        return Err(ModelError::new(
            "model_stage_insecure",
            "staged model must be an owner-matched non-writable real directory",
        ));
    }
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| ModelError::io("resolve staged model", error))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(ModelError::new(
            "model_stage_invalid",
            "staged model resolves outside the trusted root",
        ));
    }
    Ok(candidate)
}

#[cfg(not(unix))]
fn trusted_stage_candidate(_root: &Path, _stage_id: &str) -> ModelResult<PathBuf> {
    Err(ModelError::new(
        "model_stage_unsupported",
        "local model staging requires Unix filesystem security",
    ))
}

fn encode_page_token(identity: &ModelIdentity) -> String {
    URL_SAFE_NO_PAD.encode(format!(
        "{}\0{}\0{}",
        identity.model_id, identity.version, identity.revision
    ))
}

fn decode_page_token(token: &str) -> ModelResult<Option<ModelIdentity>> {
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() > 1024 {
        return Err(ModelError::new(
            "model_page_token_invalid",
            "page token is invalid",
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| ModelError::new("model_page_token_invalid", "page token is invalid"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ModelError::new("model_page_token_invalid", "page token is invalid"))?;
    let parts = text.split('\0').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| !valid_bounded_token(part)) {
        return Err(ModelError::new(
            "model_page_token_invalid",
            "page token is invalid",
        ));
    }
    Ok(Some(ModelIdentity {
        model_id: parts[0].to_string(),
        version: parts[1].to_string(),
        revision: parts[2].to_string(),
    }))
}

fn hash_part(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}
fn hash_identity(hash: &mut Sha256, identity: &ModelIdentity) {
    hash_part(hash, identity.model_id.as_bytes());
    hash_part(hash, identity.version.as_bytes());
    hash_part(hash, identity.revision.as_bytes());
}

fn model_snapshot(
    model: InstalledModel,
    observation: ModelObservation,
    runtime_available: bool,
    health: ModelHealth,
    error: Option<ErrorDetail>,
    observed_at_epoch_ms: i64,
) -> ModelSnapshot {
    ModelSnapshot {
        identity: Some(RpcModelIdentity {
            model_id: model.identity.model_id,
            version: model.identity.version,
            revision: model.identity.revision,
        }),
        lifecycle_state: match model.state {
            ModelState::Installed => ModelLifecycleState::Installed,
            ModelState::Ready => ModelLifecycleState::Ready,
            ModelState::Active => ModelLifecycleState::Active,
            ModelState::Retired => ModelLifecycleState::Retired,
            ModelState::Failed => ModelLifecycleState::Failed,
        } as i32,
        capabilities: model.capabilities,
        runtime: model.runtime,
        runtime_available,
        loaded: observation.loaded,
        active_capabilities: observation.active_capabilities,
        previous_capabilities: observation.previous_capabilities,
        generation: observation.generation,
        in_flight_tasks: u64::try_from(observation.in_flight_tasks).unwrap_or(u64::MAX),
        health: health as i32,
        manifest_sha256: model.manifest_sha256,
        error,
        observed_at_epoch_ms,
    }
}

fn error_detail(code: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: code.to_string(),
        metadata: Default::default(),
    }
}
fn mutation_success(
    operation_id: &str,
    replayed: bool,
    observed: i64,
    model: Option<ModelSnapshot>,
) -> ModelMutationResponse {
    ModelMutationResponse {
        operation_id: operation_id.to_string(),
        outcome: ModelOperationOutcome::Succeeded as i32,
        error: None,
        observed_at_epoch_ms: observed,
        model,
        replayed,
    }
}
fn mutation_failure(
    operation_id: &str,
    code: &str,
    replayed: bool,
    observed: i64,
    model: Option<ModelSnapshot>,
) -> ModelMutationResponse {
    ModelMutationResponse {
        operation_id: operation_id.to_string(),
        outcome: ModelOperationOutcome::Failed as i32,
        error: Some(error_detail(code)),
        observed_at_epoch_ms: observed,
        model,
        replayed,
    }
}
fn list_failure(code: &str) -> ListModelsResponse {
    ListModelsResponse {
        models: Vec::new(),
        next_page_token: String::new(),
        error: Some(error_detail(code)),
        observed_at_epoch_ms: now_epoch_ms(),
    }
}
fn inspect_failure(code: &str) -> InspectModelResponse {
    InspectModelResponse {
        model: None,
        error: Some(error_detail(code)),
        observed_at_epoch_ms: now_epoch_ms(),
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
    use gmv_nodec::component_management::{ComponentManagementRpc, OwnedUdsListener};
    use gmv_protocol::avai::model_management::v1::avai_model_management_server::AvaiModelManagementServer;
    use gmv_protocol::component_management::v1::component_management_server::ComponentManagementServer;

    let owned = OwnedUdsListener::bind(socket).await?;
    let incoming = owned.incoming();
    let result = tonic::transport::Server::builder()
        .add_service(ComponentManagementServer::new(ComponentManagementRpc::new(
            owner,
        )))
        .add_service(AvaiModelManagementServer::new(model_rpc))
        .serve_with_incoming_shutdown(incoming, async move { cancel.cancelled().await })
        .await;
    owned.cleanup()?;
    result.map_err(|error| base::exception::GlobalError::from_external_error(error, |_| {}))
}
