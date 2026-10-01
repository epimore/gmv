use base::sha2::{Digest, Sha256};
use gmv_protocol::{
    avai::model_management::v1::{
        ModelIdentity as RpcModelIdentity, ModelMutationResponse, ModelOperationOutcome,
        ModelSnapshot,
    },
    common::v1::{ModelDeliveryCorrelation, ModelVariantSelector, OperationRef},
};

use crate::model::{
    ClaimOperation, InstalledModel, ModelError, ModelIdentity, ModelRepository, ModelResult,
    ModelState, OperationClaimRequest, OperationReceipt, OperationReceiptLimits,
    OperationReceiptState, RuntimeCallContext, verify_package_for_selector,
};

use super::validation::{
    ensure_before_deadline, rpc_identity, runtime_context_from_epoch, runtime_unavailable,
    trusted_stage_candidate, valid_sha256, valid_stage_id, validate_new_deadline,
    validate_operation_identity,
};
use super::{AvaiModelManagementRpc, error_detail, now_epoch_ms};

impl AvaiModelManagementRpc {
    pub(super) async fn execute_mutation(
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

#[derive(Clone, Copy)]
pub(super) enum MutationKind {
    Preload,
    Activate,
    Unload,
}

pub(super) async fn mutation_for_identity(
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

pub(super) enum MutationCommand {
    Import {
        stage_id: String,
        identity: ModelIdentity,
        manifest_sha256: String,
        correlation: ModelDeliveryCorrelation,
        selector: ModelVariantSelector,
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

    pub(super) fn request_hash(&self, deadline_epoch_ms: i64) -> String {
        let mut hash = Sha256::new();
        hash_part(&mut hash, self.kind().as_bytes());
        hash_part(&mut hash, &deadline_epoch_ms.to_be_bytes());
        match self {
            Self::Import {
                stage_id,
                identity,
                manifest_sha256,
                correlation,
                selector,
            } => {
                hash_part(&mut hash, stage_id.as_bytes());
                hash_identity(&mut hash, identity);
                hash_part(&mut hash, manifest_sha256.as_bytes());
                hash_correlation(&mut hash, correlation);
                hash_selector(&mut hash, selector);
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
                selector,
                ..
            } => match service.repository.get(identity).await? {
                Some(model)
                    if model.manifest_sha256.eq_ignore_ascii_case(manifest_sha256)
                        && model.selected_variant.as_ref().is_some_and(|selected| {
                            selected.runtime == selector.runtime
                                && selected.runtime_contract_version
                                    == selector.runtime_contract_version
                                && selected.architecture == selector.architecture
                                && selected.accelerator == selector.accelerator
                        }) =>
                {
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
                selector,
                ..
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
                        && existing.selected_variant.as_ref().is_some_and(|selected| {
                            selected.runtime == selector.runtime
                                && selected.runtime_contract_version
                                    == selector.runtime_contract_version
                                && selected.architecture == selector.architecture
                                && selected.accelerator == selector.accelerator
                        }) {
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
                let expected_selector = selector.clone();
                let package = base::tokio::task::spawn_blocking(move || {
                    verify_package_for_selector(&candidate, &policy, &expected_selector)
                })
                .await
                .map_err(|error| ModelError::new("model_verify_failed", error.to_string()))??;
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
                service
                    .manager
                    .validate_selector(&package.selected_variant)?;
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

fn hash_part(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}
fn hash_identity(hash: &mut Sha256, identity: &ModelIdentity) {
    hash_part(hash, identity.model_id.as_bytes());
    hash_part(hash, identity.version.as_bytes());
    hash_part(hash, identity.revision.as_bytes());
}

fn hash_correlation(hash: &mut Sha256, correlation: &ModelDeliveryCorrelation) {
    hash_part(hash, correlation.deployment_id.as_bytes());
    hash_part(hash, &correlation.target_ordinal.to_be_bytes());
    hash_part(hash, correlation.installation_id.as_bytes());
    hash_part(hash, correlation.host_id.as_bytes());
    hash_part(hash, correlation.component_id.as_bytes());
    hash_part(hash, correlation.assignment_id.as_bytes());
}

fn hash_selector(hash: &mut Sha256, selector: &ModelVariantSelector) {
    hash_part(hash, selector.runtime.as_bytes());
    hash_part(hash, &selector.runtime_contract_version.to_be_bytes());
    hash_part(hash, selector.architecture.as_bytes());
    hash_part(hash, selector.accelerator.as_bytes());
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
pub(super) fn mutation_failure(
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
