use gmv_protocol::{
    avai::model_management::v1::{
        ActivateModelRequest, GetManagementCapabilitiesRequest, GetManagementCapabilitiesResponse,
        ImportStagedModelRequest, InspectModelRequest, InspectModelResponse, ListModelsRequest,
        ListModelsResponse, ModelCapabilityGeneration, ModelHealth,
        ModelIdentity as RpcModelIdentity, ModelLifecycleState, ModelMutationResponse,
        ModelSnapshot, PreloadModelRequest, RollbackModelRequest, UnloadModelRequest,
        avai_model_management_server::AvaiModelManagement,
    },
    common::v1::{ErrorDetail, ModelVariantSelector},
};
use tonic::{Request, Response, Status};

use crate::model::{InstalledModel, ModelObservation, ModelState, RuntimeCallContext};

use super::mutation::{MutationCommand, MutationKind, mutation_failure, mutation_for_identity};
use super::validation::{
    MAX_DEADLINE_AHEAD_MS, decode_page_token, encode_page_token, rpc_identity,
    runtime_context_from_epoch, validate_correlation, validate_selector,
};
use super::{AvaiModelManagementRpc, error_detail, now_epoch_ms};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 200;
const EXACT_MODEL_IMPORT_CONTRACT_VERSION: u32 = 1;
const EXACT_MODEL_OBSERVATION_CONTRACT_VERSION: u32 = 1;

impl AvaiModelManagementRpc {
    pub(super) async fn snapshot(
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
}

#[tonic::async_trait]
impl AvaiModelManagement for AvaiModelManagementRpc {
    async fn get_management_capabilities(
        &self,
        _request: Request<GetManagementCapabilitiesRequest>,
    ) -> Result<Response<GetManagementCapabilitiesResponse>, Status> {
        Ok(Response::new(GetManagementCapabilitiesResponse {
            exact_model_import_contract_version: EXACT_MODEL_IMPORT_CONTRACT_VERSION,
            exact_model_observation_contract_version: EXACT_MODEL_OBSERVATION_CONTRACT_VERSION,
        }))
    }

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
        let correlation = match validate_correlation(request.correlation) {
            Ok(correlation) => correlation,
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
        let selector = match validate_selector(request.expected_selector) {
            Ok(selector) => selector,
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
                    correlation,
                    selector,
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

fn model_snapshot(
    model: InstalledModel,
    observation: ModelObservation,
    runtime_available: bool,
    health: ModelHealth,
    error: Option<ErrorDetail>,
    observed_at_epoch_ms: i64,
) -> ModelSnapshot {
    let selected_variant = model
        .selected_variant
        .as_ref()
        .map(|variant| ModelVariantSelector {
            runtime: variant.runtime.clone(),
            runtime_contract_version: variant.runtime_contract_version,
            architecture: variant.architecture.clone(),
            accelerator: variant.accelerator.clone(),
        });
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
        selected_variant,
        active_bindings: observation
            .active_bindings
            .into_iter()
            .map(|binding| ModelCapabilityGeneration {
                capability: binding.capability,
                generation: binding.generation,
            })
            .collect(),
        previous_bindings: observation
            .previous_bindings
            .into_iter()
            .map(|binding| ModelCapabilityGeneration {
                capability: binding.capability,
                generation: binding.generation,
            })
            .collect(),
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
