use std::{
    future::Future,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base::tokio::sync::Semaphore;
use gmv_protocol::avai::external_provider::v1::{
    self as wire, CallOutcome, CancelOutcome, HealthOutcome, LoadOutcome, ProviderErrorKind,
    UnloadOutcome, avai_external_runtime_provider_client::AvaiExternalRuntimeProviderClient,
};
use hyper_util::rt::TokioIo;
use tonic::{Response, Status, transport::Channel};
use tower::service_fn;

use super::{
    ExecutionContract, InferenceResult, InstalledModel, ModelError, ModelIdentity, ModelInstance,
    ModelResult, RuntimeCallContext, RuntimeDescriptor, RuntimeInput, RuntimeProvider,
    SelfTestCase,
    package::{actual_model, load_installed_execution_contract, safe_relative_path},
    runtime::{RuntimeFuture, compare_json_numeric, validate_tensor_json},
};

const PROTOCOL_MAJOR: u32 = 1;
const PROTOCOL_MINOR: u32 = 0;
const MAX_EXECUTION_CALLS: usize = 64;
const MAX_CONTROL_CALLS: usize = 16;
const MAX_SUPPORTS: usize = 16;
const MAX_ID_BYTES: usize = 128;
const PROTOCOL_OVERHEAD_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ExternalProviderConfig {
    pub socket_path: PathBuf,
    pub expected_uid: u32,
    pub provider_id: String,
    pub runtime_id: String,
    pub runtime_contract_version: u32,
    pub max_execution_calls: usize,
    pub max_input_bytes: usize,
    pub max_result_bytes: usize,
    pub connect_budget: Duration,
    pub cancel_rpc_budget: Duration,
    pub cancel_drain_grace: Duration,
}

impl ExternalProviderConfig {
    fn validate(&self) -> ModelResult<()> {
        validate_socket_path(&self.socket_path, self.expected_uid)?;
        for value in [&self.provider_id, &self.runtime_id] {
            validate_id(value)?;
        }
        if self.runtime_contract_version == 0
            || self.max_execution_calls == 0
            || self.max_execution_calls > MAX_EXECUTION_CALLS
            || self.max_input_bytes == 0
            || self.max_result_bytes == 0
            || self.connect_budget.is_zero()
            || self.connect_budget > Duration::from_secs(5)
            || self.cancel_rpc_budget.is_zero()
            || self.cancel_rpc_budget > Duration::from_secs(1)
            || self.cancel_drain_grace.is_zero()
            || self.cancel_drain_grace > Duration::from_secs(5)
        {
            return Err(ModelError::new(
                "invalid_model_runtime_config",
                "external provider bounds are invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ExternalRuntimeProvider {
    config: ExternalProviderConfig,
    session: Arc<Session>,
}

struct Session {
    channel: Channel,
    provider_id: String,
    provider_instance_id: String,
    client_session_id: String,
    max_input_bytes: usize,
    max_result_bytes: usize,
    execution: Arc<Semaphore>,
    control: Arc<Semaphore>,
    cancel_rpc_budget: Duration,
    cancel_drain_grace: Duration,
    fenced: AtomicBool,
}

impl ExternalRuntimeProvider {
    pub async fn connect(config: ExternalProviderConfig) -> ModelResult<Self> {
        config.validate()?;
        let socket_path = config.socket_path.clone();
        let expected_uid = config.expected_uid;
        let connector = service_fn(move |_| {
            let socket_path = socket_path.clone();
            async move {
                let stream = base::tokio::net::UnixStream::connect(socket_path).await?;
                let peer = stream.peer_cred()?;
                if peer.uid() != expected_uid {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "external provider peer UID mismatch",
                    ));
                }
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        });
        let endpoint = tonic::transport::Endpoint::from_static("http://[::]:50051")
            .connect_timeout(config.connect_budget);
        let channel = endpoint
            .connect_with_connector(connector)
            .await
            .map_err(|_| provider_unavailable())?;
        let client_session_id = uuid::Uuid::new_v4().to_string();
        let mut client = rpc_client(
            channel.clone(),
            config.max_input_bytes,
            config.max_result_bytes,
        );
        let response = base::tokio::time::timeout(
            config.connect_budget,
            client.describe(wire::DescribeRequest {
                protocol_major: PROTOCOL_MAJOR,
                min_minor: PROTOCOL_MINOR,
                max_minor: PROTOCOL_MINOR,
                provider_id: config.provider_id.clone(),
                client_session_id: client_session_id.clone(),
            }),
        )
        .await
        .map_err(|_| provider_unavailable())?
        .map_err(|_| provider_unavailable())?
        .into_inner();
        validate_describe(&config, &client_session_id, &response)?;
        let bounds = response.bounds.ok_or_else(protocol_mismatch)?;
        if bounds.max_in_flight == 0
            || bounds.max_in_flight as usize > MAX_EXECUTION_CALLS
            || bounds.max_loaded_handles == 0
        {
            return Err(protocol_mismatch());
        }
        let peer_execution =
            usize::try_from(bounds.max_in_flight).map_err(|_| protocol_mismatch())?;
        let peer_input =
            usize::try_from(bounds.max_input_bytes).map_err(|_| protocol_mismatch())?;
        let peer_result =
            usize::try_from(bounds.max_result_bytes).map_err(|_| protocol_mismatch())?;
        let execution_limit = config.max_execution_calls.min(peer_execution);
        let max_input_bytes = config.max_input_bytes.min(peer_input);
        let max_result_bytes = config.max_result_bytes.min(peer_result);
        if execution_limit == 0 || max_input_bytes == 0 || max_result_bytes == 0 {
            return Err(protocol_mismatch());
        }
        base::log::info!(
            "External provider connected: action=model_runtime, stage=connect, outcome=succeeded, runtime={}, provider={}",
            config.runtime_id,
            config.provider_id
        );
        Ok(Self {
            config: config.clone(),
            session: Arc::new(Session {
                channel,
                provider_id: response.provider_id,
                provider_instance_id: response.provider_instance_id,
                client_session_id,
                max_input_bytes,
                max_result_bytes,
                execution: Arc::new(Semaphore::new(execution_limit)),
                control: Arc::new(Semaphore::new(
                    MAX_CONTROL_CALLS.min(execution_limit.max(1)),
                )),
                cancel_rpc_budget: config.cancel_rpc_budget,
                cancel_drain_grace: config.cancel_drain_grace,
                fenced: AtomicBool::new(false),
            }),
        })
    }
}

fn validate_external_selector(
    selector: &super::RuntimeVariant,
    runtime_contract_version: u32,
) -> ModelResult<()> {
    if selector.runtime_contract_version != runtime_contract_version {
        return Err(ModelError::new(
            "model_runtime_contract_unsupported",
            "external provider does not support the requested runtime contract",
        ));
    }
    if !selector.accelerator.is_empty() {
        return Err(ModelError::new(
            "model_accelerator_unavailable",
            "external provider v1 cannot prove accelerator support",
        ));
    }
    Ok(())
}

impl RuntimeProvider for ExternalRuntimeProvider {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            runtime: self.config.runtime_id.clone(),
            version: format!("external-v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}"),
        }
    }

    fn validate_selector(&self, selector: &super::RuntimeVariant) -> ModelResult<()> {
        validate_external_selector(selector, self.config.runtime_contract_version)
    }

    fn preload<'a>(
        &'a self,
        model: &'a InstalledModel,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, Arc<dyn ModelInstance>> {
        Box::pin(async move {
            self.session.ensure_live()?;
            context.ensure_active()?;
            let installed = load_installed_execution_contract(model)?;
            if installed.selected_variant.runtime != self.config.runtime_id
                || installed.selected_variant.runtime_contract_version
                    != self.config.runtime_contract_version
            {
                return Err(ModelError::new(
                    "model_runtime_contract_mismatch",
                    "selected variant does not match the configured external provider",
                ));
            }
            let execution = installed.execution.ok_or_else(|| {
                ModelError::new(
                    "model_runtime_contract_mismatch",
                    "external provider requires execution contract v1",
                )
            })?;
            let artifact = safe_relative_path(&installed.selected_variant.artifact)?;
            let artifact_size = std::fs::symlink_metadata(model.installed_path.join(&artifact))
                .map_err(|error| ModelError::io("inspect selected model artifact", error))?
                .len();
            let call_id = new_call_id();
            let request = wire::LoadModelRequest {
                fence: Some(self.session.fence(&call_id, "")),
                timeout_ms: remaining_timeout_ms(&context)?,
                model: Some(to_wire_identity(&model.identity)),
                runtime_id: self.config.runtime_id.clone(),
                runtime_contract_version: self.config.runtime_contract_version,
                artifact_relative_path: installed.selected_variant.artifact.clone(),
                artifact_sha256: installed.selected_variant.artifact_sha256.clone(),
                artifact_size,
                execution: Some(to_wire_execution(&execution)),
                result: Some(wire::ResultContract {
                    schema_name: installed.result_schema.name.clone(),
                    schema_version: installed.result_schema.version,
                    max_result_bytes: self.session.max_result_bytes as u64,
                }),
            };
            let _permit = self
                .session
                .execution
                .clone()
                .try_acquire_owned()
                .map_err(|_| runtime_busy())?;
            let mut client = self.session.client();
            let response = self
                .session
                .await_rpc(&context, &call_id, "", async move {
                    client.load_model(request).await
                })
                .await?;
            let response = response.into_inner();
            let handle = response
                .fence
                .as_ref()
                .map(|fence| fence.load_handle_id.as_str())
                .unwrap_or_default();
            self.session
                .validate_fence(response.fence.as_ref(), &call_id, handle)?;
            if !is_safe_id(handle) {
                self.session
                    .mark_fenced("load", "model_runtime_protocol_violation");
                return Err(ModelError::new(
                    "model_runtime_protocol_violation",
                    "external provider returned an invalid load handle",
                ));
            }
            if response.outcome != LoadOutcome::Loaded as i32 {
                return Err(map_provider_error(response.error, "model_preload_failed"));
            }
            Ok(Arc::new(ExternalModelInstance {
                session: self.session.clone(),
                identity: model.identity.clone(),
                runtime: self.config.runtime_id.clone(),
                capabilities: model.capabilities.clone(),
                installed_path: model.installed_path.clone(),
                execution,
                self_tests: installed.self_tests,
                load_handle_id: handle.to_string(),
                lane: Arc::new(Semaphore::new(1)),
                accepting: AtomicBool::new(true),
                unloaded: AtomicBool::new(false),
            }) as Arc<dyn ModelInstance>)
        })
    }
}

struct ExternalModelInstance {
    session: Arc<Session>,
    identity: ModelIdentity,
    runtime: String,
    capabilities: Vec<String>,
    installed_path: PathBuf,
    execution: ExecutionContract,
    self_tests: Vec<SelfTestCase>,
    load_handle_id: String,
    lane: Arc<Semaphore>,
    accepting: AtomicBool,
    unloaded: AtomicBool,
}

impl ModelInstance for ExternalModelInstance {
    fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    fn runtime(&self) -> &str {
        &self.runtime
    }

    fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    fn unload<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            self.session.ensure_live()?;
            let timeout_ms = remaining_timeout_ms(&context)?;
            if self.unloaded.load(Ordering::Acquire) {
                return Ok(());
            }
            if !self.accepting.swap(false, Ordering::AcqRel) {
                return Err(runtime_busy());
            }
            let lane = match self.lane.clone().try_acquire_owned() {
                Ok(lane) => lane,
                Err(_) => {
                    self.accepting.store(true, Ordering::Release);
                    return Err(runtime_busy());
                }
            };
            let call_id = new_call_id();
            let request = wire::UnloadModelRequest {
                fence: Some(self.session.fence(&call_id, &self.load_handle_id)),
                timeout_ms,
            };
            let _global = match self.session.execution.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    self.accepting.store(true, Ordering::Release);
                    return Err(runtime_busy());
                }
            };
            let mut client = self.session.client();
            let result = self
                .session
                .await_rpc(&context, &call_id, &self.load_handle_id, async move {
                    client.unload_model(request).await
                })
                .await
                .and_then(|response| {
                    let response = response.into_inner();
                    self.session.validate_fence(
                        response.fence.as_ref(),
                        &call_id,
                        &self.load_handle_id,
                    )?;
                    if matches!(
                        UnloadOutcome::try_from(response.outcome),
                        Ok(UnloadOutcome::Unloaded | UnloadOutcome::AlreadyUnloaded)
                    ) {
                        Ok(())
                    } else {
                        Err(map_provider_error(
                            response.error,
                            "model_runtime_provider_unavailable",
                        ))
                    }
                });
            drop(lane);
            if result.is_ok() {
                self.unloaded.store(true, Ordering::Release);
            } else if !self.session.fenced.load(Ordering::Acquire) {
                self.accepting.store(true, Ordering::Release);
            }
            result
        })
    }

    fn self_test<'a>(
        &'a self,
        cases: &'a [SelfTestCase],
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            let requested = base::serde_json::to_vec(cases)
                .map_err(|error| ModelError::io("encode requested model self-tests", error))?;
            let installed = base::serde_json::to_vec(&self.self_tests)
                .map_err(|error| ModelError::io("encode installed model self-tests", error))?;
            if requested != installed || self.self_tests.is_empty() {
                return Err(ModelError::new(
                    "model_self_test_contract_missing",
                    "durable self-tests do not match the immutable manifest",
                ));
            }
            for case in &self.self_tests {
                let input_path = safe_relative_path(&case.input)?;
                let expected_path = safe_relative_path(&case.expected)?;
                let encoded = std::fs::read(self.installed_path.join(&input_path))
                    .map_err(|error| ModelError::io("read model self-test input", error))?;
                let image = image::load_from_memory(&encoded).map_err(|_| {
                    ModelError::new("model_self_test_failed", "self-test input is not an image")
                })?;
                let result = self
                    .infer(
                        RuntimeInput {
                            encoded: encoded.into(),
                            media_type: media_type_from_path(&input_path)?.to_string(),
                            width: image.width(),
                            height: image.height(),
                        },
                        context.clone(),
                    )
                    .await?;
                let expected = std::fs::read(self.installed_path.join(expected_path))
                    .map_err(|error| ModelError::io("read model self-test oracle", error))?;
                compare_json_numeric(
                    &result.output,
                    &expected,
                    case.oracle.as_ref().ok_or_else(|| {
                        ModelError::new(
                            "model_self_test_contract_missing",
                            "self-test numeric oracle is missing",
                        )
                    })?,
                )?;
            }
            Ok(())
        })
    }

    fn health<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            let (_lane, _global) = self.admit_execution()?;
            let call_id = new_call_id();
            let request = wire::HealthRequest {
                fence: Some(self.session.fence(&call_id, &self.load_handle_id)),
                timeout_ms: remaining_timeout_ms(&context)?,
            };
            let mut client = self.session.client();
            let response = self
                .session
                .await_rpc(&context, &call_id, &self.load_handle_id, async move {
                    client.health(request).await
                })
                .await?
                .into_inner();
            self.session
                .validate_fence(response.fence.as_ref(), &call_id, &self.load_handle_id)?;
            match HealthOutcome::try_from(response.outcome) {
                Ok(HealthOutcome::Healthy) => Ok(()),
                Ok(HealthOutcome::Busy) => Err(runtime_busy()),
                Ok(HealthOutcome::Unhealthy) => Err(ModelError::new(
                    "model_health_failed",
                    "external provider reported unhealthy",
                )),
                _ => Err(map_provider_error(
                    response.error,
                    "model_runtime_protocol_violation",
                )),
            }
        })
    }

    fn infer<'a>(
        &'a self,
        input: RuntimeInput,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, InferenceResult> {
        Box::pin(async move {
            if input.encoded.len() > self.session.max_input_bytes {
                return Err(ModelError::new(
                    "model_runtime_contract_mismatch",
                    "runtime input exceeds the configured provider limit",
                ));
            }
            let (_lane, _global) = self.admit_execution()?;
            let call_id = new_call_id();
            let request = wire::InferRequest {
                fence: Some(self.session.fence(&call_id, &self.load_handle_id)),
                timeout_ms: remaining_timeout_ms(&context)?,
                input: Some(wire::RuntimeInput {
                    encoded: input.encoded.to_vec(),
                    media_type: input.media_type,
                    width: input.width,
                    height: input.height,
                }),
            };
            let mut client = self.session.client();
            let response = self
                .session
                .await_rpc(&context, &call_id, &self.load_handle_id, async move {
                    client.infer(request).await
                })
                .await?
                .into_inner();
            self.session
                .validate_fence(response.fence.as_ref(), &call_id, &self.load_handle_id)?;
            match CallOutcome::try_from(response.outcome) {
                Ok(CallOutcome::Succeeded) => {
                    if response.tensor_json.len() > self.session.max_result_bytes {
                        return Err(ModelError::new(
                            "model_runtime_response_invalid",
                            "provider result exceeds the configured limit",
                        ));
                    }
                    validate_tensor_json(&response.tensor_json, &self.execution)?;
                    Ok(InferenceResult {
                        output: response.tensor_json,
                        actual_model: actual_model(&self.identity, self.runtime.clone()),
                    })
                }
                Ok(CallOutcome::Busy) => Err(runtime_busy()),
                Ok(CallOutcome::Cancelled) => Err(ModelError::new(
                    "model_runtime_cancelled",
                    "external provider call was cancelled",
                )),
                _ => Err(map_provider_error(
                    response.error,
                    "model_runtime_provider_unavailable",
                )),
            }
        })
    }
}

impl ExternalModelInstance {
    fn admit_execution(
        &self,
    ) -> ModelResult<(
        base::tokio::sync::OwnedSemaphorePermit,
        base::tokio::sync::OwnedSemaphorePermit,
    )> {
        self.session.ensure_live()?;
        if !self.accepting.load(Ordering::Acquire) || self.unloaded.load(Ordering::Acquire) {
            return Err(ModelError::new(
                "model_runtime_stale_handle",
                "external provider handle is closed",
            ));
        }
        let lane = self
            .lane
            .clone()
            .try_acquire_owned()
            .map_err(|_| runtime_busy())?;
        let global = self
            .session
            .execution
            .clone()
            .try_acquire_owned()
            .map_err(|_| runtime_busy())?;
        Ok((lane, global))
    }
}

impl Session {
    fn mark_fenced(&self, stage: &str, error_code: &str) {
        if self
            .fenced
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            base::log::warn!(
                "External provider session fenced: action=model_runtime, stage={}, outcome=failed, provider={}, error_code={}",
                stage,
                self.provider_id,
                error_code
            );
        } else {
            base::log::debug!(
                "External provider session remains fenced: action=model_runtime, stage={}, outcome=replayed, provider={}, error_code={}",
                stage,
                self.provider_id,
                error_code
            );
        }
    }

    fn client(&self) -> AvaiExternalRuntimeProviderClient<Channel> {
        rpc_client(
            self.channel.clone(),
            self.max_input_bytes,
            self.max_result_bytes,
        )
    }

    fn ensure_live(&self) -> ModelResult<()> {
        if self.fenced.load(Ordering::Acquire) {
            Err(ModelError::new(
                "model_runtime_stale_handle",
                "external provider session is fenced",
            ))
        } else {
            Ok(())
        }
    }

    fn fence(&self, call_id: &str, load_handle_id: &str) -> wire::Fence {
        wire::Fence {
            provider_id: self.provider_id.clone(),
            provider_instance_id: self.provider_instance_id.clone(),
            client_session_id: self.client_session_id.clone(),
            call_id: call_id.to_string(),
            load_handle_id: load_handle_id.to_string(),
        }
    }

    fn validate_fence(
        &self,
        actual: Option<&wire::Fence>,
        call_id: &str,
        load_handle_id: &str,
    ) -> ModelResult<()> {
        let valid = actual.is_some_and(|actual| {
            actual.provider_id == self.provider_id
                && actual.provider_instance_id == self.provider_instance_id
                && actual.client_session_id == self.client_session_id
                && actual.call_id == call_id
                && actual.load_handle_id == load_handle_id
        });
        if valid {
            Ok(())
        } else {
            self.mark_fenced("validate_fence", "model_runtime_stale_handle");
            Err(ModelError::new(
                "model_runtime_stale_handle",
                "external provider response identity mismatch",
            ))
        }
    }

    async fn await_rpc<T, F>(
        &self,
        context: &RuntimeCallContext,
        call_id: &str,
        load_handle_id: &str,
        future: F,
    ) -> ModelResult<Response<T>>
    where
        F: Future<Output = Result<Response<T>, Status>>,
    {
        self.ensure_live()?;
        context.ensure_active()?;
        let deadline = base::tokio::time::Instant::from_std(context.deadline);
        base::tokio::pin!(future);
        let primary = base::tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => ModelError::new(
                "model_runtime_cancelled",
                "runtime call was cancelled",
            ),
            _ = base::tokio::time::sleep_until(deadline) => ModelError::new(
                "model_runtime_deadline_exceeded",
                "runtime call deadline expired",
            ),
            response = &mut future => return self.map_transport(response),
        };
        self.cancel_call(call_id, load_handle_id).await;
        if base::tokio::time::timeout(self.cancel_drain_grace, &mut future)
            .await
            .is_err()
        {
            self.mark_fenced("cancel_drain", "model_runtime_stale_handle");
        }
        Err(primary)
    }

    async fn cancel_call(&self, target_call_id: &str, load_handle_id: &str) {
        let Ok(_permit) = self.control.clone().try_acquire_owned() else {
            self.mark_fenced("cancel_admission", "model_runtime_stale_handle");
            return;
        };
        let call_id = new_call_id();
        let request = wire::CancelRequest {
            fence: Some(self.fence(&call_id, load_handle_id)),
            target_call_id: target_call_id.to_string(),
        };
        let mut client = self.client();
        let response =
            base::tokio::time::timeout(self.cancel_rpc_budget, client.cancel(request)).await;
        let Ok(Ok(response)) = response else {
            self.mark_fenced("cancel", "model_runtime_stale_handle");
            return;
        };
        let response = response.into_inner();
        if self
            .validate_fence(response.fence.as_ref(), &call_id, load_handle_id)
            .is_err()
            || !matches!(
                CancelOutcome::try_from(response.outcome),
                Ok(CancelOutcome::Accepted | CancelOutcome::AlreadyTerminal)
            )
        {
            self.mark_fenced("cancel_response", "model_runtime_stale_handle");
        }
    }

    fn map_transport<T>(&self, response: Result<Response<T>, Status>) -> ModelResult<Response<T>> {
        response.map_err(|status| {
            if status.code() == tonic::Code::FailedPrecondition {
                self.mark_fenced("transport", "model_runtime_stale_handle");
                ModelError::new(
                    "model_runtime_stale_handle",
                    "external provider rejected a stale session or handle",
                )
            } else {
                self.mark_fenced("transport", "model_runtime_provider_unavailable");
                provider_unavailable()
            }
        })
    }
}

fn rpc_client(
    channel: Channel,
    max_input_bytes: usize,
    max_result_bytes: usize,
) -> AvaiExternalRuntimeProviderClient<Channel> {
    AvaiExternalRuntimeProviderClient::new(channel)
        .max_encoding_message_size(max_input_bytes.saturating_add(PROTOCOL_OVERHEAD_BYTES))
        .max_decoding_message_size(max_result_bytes.saturating_add(PROTOCOL_OVERHEAD_BYTES))
}

fn validate_describe(
    config: &ExternalProviderConfig,
    client_session_id: &str,
    response: &wire::DescribeResponse,
) -> ModelResult<()> {
    if response.selected_major != PROTOCOL_MAJOR
        || response.selected_minor != PROTOCOL_MINOR
        || response.provider_id != config.provider_id
        || response.client_session_id != client_session_id
        || response.provider_instance_id.is_empty()
        || response.implementation_version.is_empty()
        || response.supported_runtimes.is_empty()
        || response.supported_runtimes.len() > MAX_SUPPORTS
        || response
            .supported_runtimes
            .iter()
            .any(|support| !is_safe_id(&support.runtime_id))
        || !response.supported_runtimes.iter().any(|support| {
            support.runtime_id == config.runtime_id
                && support.runtime_contract_version == config.runtime_contract_version
        })
    {
        return Err(protocol_mismatch());
    }
    for value in [
        &response.provider_id,
        &response.provider_instance_id,
        &response.client_session_id,
        &response.implementation_version,
    ] {
        validate_id(value).map_err(|_| protocol_mismatch())?;
    }
    Ok(())
}

fn validate_socket_path(path: &Path, _expected_uid: u32) -> ModelResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(ModelError::new(
            "invalid_model_runtime_config",
            "provider socket must be an absolute normalized path",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ModelError::new(
            "invalid_model_runtime_config",
            "provider socket must have a secure parent",
        )
    })?;
    let parent_metadata = std::fs::symlink_metadata(parent).map_err(|_| {
        ModelError::new(
            "model_runtime_provider_unavailable",
            "socket parent missing",
        )
    })?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| provider_unavailable())?;
    if !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || parent_metadata.uid() != metadata.uid()
        || parent_metadata.permissions().mode() & 0o022 != 0
    {
        return Err(ModelError::new(
            "invalid_model_runtime_config",
            "provider socket parent is not secure",
        ));
    }
    if !metadata.file_type().is_socket()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(ModelError::new(
            "model_runtime_provider_unavailable",
            "provider endpoint is not a 0600 Unix socket",
        ));
    }
    Ok(())
}

fn validate_id(value: &str) -> ModelResult<()> {
    if !is_safe_id(value) {
        Err(ModelError::new(
            "model_runtime_protocol_mismatch",
            "external provider identity is invalid",
        ))
    } else {
        Ok(())
    }
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn remaining_timeout_ms(context: &RuntimeCallContext) -> ModelResult<u64> {
    context.ensure_active()?;
    let remaining = context.deadline.saturating_duration_since(Instant::now());
    let millis = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
    if millis == 0 {
        Err(ModelError::new(
            "model_runtime_deadline_exceeded",
            "runtime call deadline expired",
        ))
    } else {
        Ok(millis)
    }
}

fn to_wire_identity(identity: &ModelIdentity) -> wire::ModelIdentity {
    wire::ModelIdentity {
        model_id: identity.model_id.clone(),
        version: identity.version.clone(),
        revision: identity.revision.clone(),
    }
}

fn to_wire_tensor(tensor: &super::TensorContract) -> wire::TensorContract {
    wire::TensorContract {
        name: tensor.name.clone(),
        dtype: tensor.dtype.clone(),
        shape: tensor.shape.clone(),
        layout: tensor.layout.clone(),
    }
}

fn to_wire_execution(execution: &ExecutionContract) -> wire::ExecutionContractV1 {
    wire::ExecutionContractV1 {
        version: execution.version,
        input: Some(wire::ExecutionInputContract {
            kind: execution.input.kind.clone(),
            accepted_media_types: execution.input.accepted_media_types.clone(),
            max_bytes: execution.input.max_bytes,
            max_width: execution.input.max_width,
            max_height: execution.input.max_height,
            tensor: Some(to_wire_tensor(&execution.input.tensor)),
            preprocess: Some(wire::PreprocessContract {
                resize: execution.input.preprocess.resize.clone(),
                interpolation: execution.input.preprocess.interpolation.clone(),
                color: execution.input.preprocess.color.clone(),
                scale: execution.input.preprocess.scale,
                mean: execution.input.preprocess.mean.to_vec(),
                std: execution.input.preprocess.std.to_vec(),
            }),
        }),
        outputs: execution.outputs.iter().map(to_wire_tensor).collect(),
        postprocess_kind: execution.postprocess.kind.clone(),
    }
}

fn media_type_from_path(path: &Path) -> ModelResult<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("png") => Ok("image/png"),
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("webp") => Ok("image/webp"),
        _ => Err(ModelError::new(
            "model_self_test_failed",
            "self-test input extension is unsupported",
        )),
    }
}

fn map_provider_error(
    error: Option<wire::ProviderError>,
    default_code: &'static str,
) -> ModelError {
    match error.and_then(|error| ProviderErrorKind::try_from(error.kind).ok()) {
        Some(ProviderErrorKind::Failed) => {
            ModelError::new(default_code, "external provider rejected the runtime call")
        }
        Some(ProviderErrorKind::Busy) => runtime_busy(),
        Some(ProviderErrorKind::ContractMismatch) => ModelError::new(
            "model_runtime_contract_mismatch",
            "external provider rejected the signed execution contract",
        ),
        Some(ProviderErrorKind::InvalidRequest) | None | Some(ProviderErrorKind::Unspecified) => {
            ModelError::new(
                "model_runtime_protocol_violation",
                "external provider returned an invalid error outcome",
            )
        }
    }
}

fn new_call_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn provider_unavailable() -> ModelError {
    ModelError::new(
        "model_runtime_provider_unavailable",
        "external runtime provider is unavailable",
    )
}

fn protocol_mismatch() -> ModelError {
    ModelError::new(
        "model_runtime_protocol_mismatch",
        "external provider protocol negotiation failed",
    )
}

fn runtime_busy() -> ModelError {
    ModelError::new(
        "model_runtime_busy",
        "external provider execution capacity is full",
    )
}

#[cfg(test)]
mod selector_tests {
    use super::*;

    #[test]
    fn external_v1_requires_exact_contract_and_unclaimed_accelerator() {
        let mut selector = super::super::RuntimeVariant {
            runtime: "external-test".into(),
            runtime_contract_version: 7,
            architecture: std::env::consts::ARCH.into(),
            accelerator: String::new(),
            artifact: "model.bin".into(),
        };
        validate_external_selector(&selector, 7).unwrap();

        selector.runtime_contract_version = 8;
        assert_eq!(
            validate_external_selector(&selector, 7).unwrap_err().code,
            "model_runtime_contract_unsupported"
        );
        selector.runtime_contract_version = 7;
        selector.accelerator = "gpu".into();
        assert_eq!(
            validate_external_selector(&selector, 7).unwrap_err().code,
            "model_accelerator_unavailable"
        );
    }
}
