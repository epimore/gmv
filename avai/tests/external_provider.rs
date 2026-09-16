use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use avai::{
    model::{
        ExternalProviderConfig, ExternalRuntimeProvider, HealthReconcile, ModelIdentity,
        ModelManager, ModelManagerConfig, ModelPackageManifest, ModelRepository, ModelState,
        PackagePolicy, RuntimeCallContext, RuntimeInput, RuntimeProvider,
        model_package_signing_payload, verify_package,
    },
    source::SourcePolicy,
    task::{TaskManager, TaskManagerConfig},
};
use base::{
    base64::Engine,
    futures::StreamExt,
    sha2::{Digest, Sha256},
    tokio::sync::{Mutex, Notify},
    tokio_util::sync::CancellationToken,
    utils::rt::GlobalRuntime,
};
use ed25519_dalek::{Signer, SigningKey};
use gmv_nodec::component_management::OwnedUdsListener;
use gmv_protocol::{
    avai::{
        external_provider::v1::{
            self as wire, CallOutcome, CancelOutcome, HealthOutcome, LoadOutcome, UnloadOutcome,
            avai_external_runtime_provider_client::AvaiExternalRuntimeProviderClient,
            avai_external_runtime_provider_server::{
                AvaiExternalRuntimeProvider as ProviderService, AvaiExternalRuntimeProviderServer,
            },
        },
        v1::{
            AiTaskState, CreateTaskRequest, ImageMetadata, OwnedImageRef, QueryTaskRequest,
            SourceSpec, source_spec,
        },
    },
    common::v1::{AccessGrant, DataEndpoint, NodeIdentity, NodeKind, OperationRef, ResourceRef},
};
use tonic::{Request, Response, Status};

const RUNTIME: &str = "external-test";
const PROVIDER_ID: &str = "test-provider";
const CAPABILITY: &str = "tensor.test";
const RESULT: &[u8] =
    br#"{"outputs":[{"name":"output","dtype":"f32","shape":[1,3,1,1],"data":[1.0,2.0,3.0]}]}"#;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);
const PROVIDER_MAX_CALL_BUDGET: Duration = Duration::from_millis(250);
const SESSION_REPLACEMENT_BUDGET: Duration = Duration::from_millis(100);
const PROVIDER_MAX_CALLS: usize = 8;

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "avai-external-provider-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct ActiveCall {
    client_session_id: String,
    cancellation: CancellationToken,
}

#[derive(Default)]
struct FakeState {
    session: Mutex<Option<String>>,
    session_transition: Mutex<()>,
    replacing_session: AtomicBool,
    handles: Mutex<HashSet<String>>,
    handle_models: Mutex<HashMap<String, String>>,
    unloaded: Mutex<HashSet<String>>,
    calls: Mutex<HashMap<String, ActiveCall>>,
    busy_handles: Mutex<HashSet<String>>,
    next_handle: AtomicUsize,
    load_calls: AtomicUsize,
    unload_calls: AtomicUsize,
    infer_calls: AtomicUsize,
    block_load: AtomicBool,
    block_infer: AtomicBool,
    block_health: AtomicBool,
    block_unload: AtomicBool,
    ignore_cancel: AtomicBool,
    hold_after_cancel: AtomicBool,
    wrong_fence: AtomicUsize,
    oversized_response: AtomicBool,
    wrong_protocol: AtomicBool,
    unhealthy: AtomicBool,
    force_already_unloaded: AtomicBool,
    unhealthy_models: Mutex<HashSet<String>>,
    call_started: Notify,
    calls_changed: Notify,
    cancel_seen: Notify,
    release: Notify,
    last_load: Mutex<Option<wire::LoadModelRequest>>,
}

#[derive(Clone)]
struct FakeProvider {
    provider_instance_id: String,
    packages_root: PathBuf,
    state: Arc<FakeState>,
}

impl FakeProvider {
    async fn validate_fence(&self, fence: Option<&wire::Fence>) -> Result<wire::Fence, Status> {
        let fence = fence
            .filter(|fence| {
                fence.provider_id == PROVIDER_ID
                    && fence.provider_instance_id == self.provider_instance_id
            })
            .cloned()
            .ok_or_else(|| Status::failed_precondition("stale provider fence"))?;
        if self.state.replacing_session.load(Ordering::Acquire) {
            return Err(Status::unavailable("provider session is being replaced"));
        }
        if self.state.session.lock().await.as_deref() != Some(&fence.client_session_id) {
            return Err(Status::failed_precondition("stale client session"));
        }
        Ok(fence)
    }

    fn response_fence(&self, mut fence: wire::Fence) -> wire::Fence {
        match self.state.wrong_fence.swap(0, Ordering::AcqRel) {
            1 => fence.call_id = "wrong-call".to_string(),
            2 => fence.client_session_id = "wrong-session".to_string(),
            3 => fence.provider_instance_id = "wrong-instance".to_string(),
            4 => fence.load_handle_id = "wrong-handle".to_string(),
            _ => {}
        }
        fence
    }

    async fn acquire_handle(&self, handle: &str) -> Result<(), Status> {
        if !self.state.handles.lock().await.contains(handle) {
            return Err(Status::failed_precondition("stale load handle"));
        }
        if !self
            .state
            .busy_handles
            .lock()
            .await
            .insert(handle.to_string())
        {
            return Err(Status::resource_exhausted("handle busy"));
        }
        Ok(())
    }

    async fn begin_call(&self, fence: &wire::Fence) -> Result<CancellationToken, Status> {
        if self.state.replacing_session.load(Ordering::Acquire) {
            return Err(Status::unavailable("provider session is being replaced"));
        }
        let cancellation = CancellationToken::new();
        let mut calls = self.state.calls.lock().await;
        if calls.len() >= PROVIDER_MAX_CALLS || calls.contains_key(&fence.call_id) {
            return Err(Status::resource_exhausted(
                "provider call capacity exhausted",
            ));
        }
        calls.insert(
            fence.call_id.clone(),
            ActiveCall {
                client_session_id: fence.client_session_id.clone(),
                cancellation: cancellation.clone(),
            },
        );
        Ok(cancellation)
    }

    async fn finish_call(&self, call_id: &str) {
        self.state.calls.lock().await.remove(call_id);
        self.state.calls_changed.notify_waiters();
    }

    fn call_deadline(timeout_ms: u64) -> Result<base::tokio::time::Instant, Status> {
        if timeout_ms == 0 {
            return Err(Status::invalid_argument("timeout_ms must be positive"));
        }
        Ok(base::tokio::time::Instant::now()
            + Duration::from_millis(timeout_ms).min(PROVIDER_MAX_CALL_BUDGET))
    }

    async fn wait_for_blocked_work(
        &self,
        cancellation: &CancellationToken,
        deadline: base::tokio::time::Instant,
    ) -> Result<(), Status> {
        if self.state.ignore_cancel.load(Ordering::Acquire) {
            return base::tokio::time::timeout_at(deadline, self.state.release.notified())
                .await
                .map_err(|_| Status::deadline_exceeded("provider-local deadline expired"));
        }
        base::tokio::select! {
            _ = cancellation.cancelled() => {}
            _ = base::tokio::time::sleep_until(deadline) => {
                return Err(Status::deadline_exceeded("provider-local deadline expired"));
            }
        }
        if self.state.hold_after_cancel.load(Ordering::Acquire) {
            base::tokio::time::timeout_at(deadline, self.state.release.notified())
                .await
                .map_err(|_| Status::deadline_exceeded("provider-local deadline expired"))?;
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl ProviderService for FakeProvider {
    async fn describe(
        &self,
        request: Request<wire::DescribeRequest>,
    ) -> Result<Response<wire::DescribeResponse>, Status> {
        let request = request.into_inner();
        if request.protocol_major != 1 || request.min_minor > 0 {
            return Err(Status::failed_precondition("protocol mismatch"));
        }
        let _transition = self.state.session_transition.lock().await;
        let current = self.state.session.lock().await.clone();
        if current.as_deref() != Some(&request.client_session_id) {
            self.state.replacing_session.store(true, Ordering::Release);
            if let Some(previous_session) = current.as_deref() {
                let drain_deadline = base::tokio::time::Instant::now() + SESSION_REPLACEMENT_BUDGET;
                loop {
                    let active = {
                        let calls = self.state.calls.lock().await;
                        calls
                            .values()
                            .filter(|call| call.client_session_id == previous_session)
                            .cloned()
                            .collect::<Vec<_>>()
                    };
                    if active.is_empty() {
                        break;
                    }
                    for call in active {
                        call.cancellation.cancel();
                    }
                    if base::tokio::time::timeout_at(
                        drain_deadline,
                        self.state.calls_changed.notified(),
                    )
                    .await
                    .is_err()
                    {
                        self.state.replacing_session.store(false, Ordering::Release);
                        return Err(Status::unavailable("previous provider session is busy"));
                    }
                }
            }
            self.state.handles.lock().await.clear();
            self.state.handle_models.lock().await.clear();
            self.state.unloaded.lock().await.clear();
            self.state.busy_handles.lock().await.clear();
            *self.state.session.lock().await = Some(request.client_session_id.clone());
            self.state.replacing_session.store(false, Ordering::Release);
        }
        Ok(Response::new(wire::DescribeResponse {
            selected_major: if self.state.wrong_protocol.load(Ordering::Acquire) {
                2
            } else {
                1
            },
            selected_minor: 0,
            provider_id: PROVIDER_ID.to_string(),
            provider_instance_id: self.provider_instance_id.clone(),
            client_session_id: request.client_session_id,
            implementation_version: "fake-v1".to_string(),
            supported_runtimes: vec![wire::RuntimeSupport {
                runtime_id: RUNTIME.to_string(),
                runtime_contract_version: 1,
            }],
            bounds: Some(wire::ProviderBounds {
                max_in_flight: 8,
                max_input_bytes: 1024 * 1024,
                max_result_bytes: 1024 * 1024,
                max_loaded_handles: 8,
            }),
        }))
    }

    async fn load_model(
        &self,
        request: Request<wire::LoadModelRequest>,
    ) -> Result<Response<wire::LoadModelResponse>, Status> {
        let request = request.into_inner();
        let deadline = Self::call_deadline(request.timeout_ms)?;
        let mut fence = self.validate_fence(request.fence.as_ref()).await?;
        validate_load_artifact(&self.packages_root, &request)?;
        let cancel = self.begin_call(&fence).await?;
        self.state.call_started.notify_waiters();
        if self.state.block_load.load(Ordering::Acquire) {
            let result = self.wait_for_blocked_work(&cancel, deadline).await;
            self.finish_call(&fence.call_id).await;
            result?;
            return Ok(Response::new(wire::LoadModelResponse {
                fence: Some(self.response_fence(fence)),
                outcome: LoadOutcome::Unspecified as i32,
                error: None,
            }));
        }
        let handle = format!(
            "handle-{}",
            self.state.next_handle.fetch_add(1, Ordering::AcqRel)
        );
        let mut handles = self.state.handles.lock().await;
        if handles.len() >= PROVIDER_MAX_CALLS {
            drop(handles);
            self.finish_call(&fence.call_id).await;
            return Err(Status::resource_exhausted(
                "provider handle capacity exhausted",
            ));
        }
        handles.insert(handle.clone());
        drop(handles);
        self.state.handle_models.lock().await.insert(
            handle.clone(),
            request.model.as_ref().unwrap().model_id.clone(),
        );
        fence.load_handle_id = handle;
        self.state.load_calls.fetch_add(1, Ordering::AcqRel);
        *self.state.last_load.lock().await = Some(request);
        self.finish_call(&fence.call_id).await;
        Ok(Response::new(wire::LoadModelResponse {
            fence: Some(self.response_fence(fence)),
            outcome: LoadOutcome::Loaded as i32,
            error: None,
        }))
    }

    async fn unload_model(
        &self,
        request: Request<wire::UnloadModelRequest>,
    ) -> Result<Response<wire::UnloadModelResponse>, Status> {
        let request = request.into_inner();
        let deadline = Self::call_deadline(request.timeout_ms)?;
        let fence = self.validate_fence(request.fence.as_ref()).await?;
        if self
            .state
            .busy_handles
            .lock()
            .await
            .contains(&fence.load_handle_id)
        {
            return Err(Status::resource_exhausted("handle busy"));
        }
        let cancel = self.begin_call(&fence).await?;
        self.state.call_started.notify_waiters();
        if self.state.block_unload.load(Ordering::Acquire) {
            let result = self.wait_for_blocked_work(&cancel, deadline).await;
            self.finish_call(&fence.call_id).await;
            result?;
            if cancel.is_cancelled() {
                return Ok(Response::new(wire::UnloadModelResponse {
                    fence: Some(self.response_fence(fence)),
                    outcome: UnloadOutcome::Unspecified as i32,
                    error: None,
                }));
            }
        }
        if base::tokio::time::Instant::now() >= deadline {
            self.finish_call(&fence.call_id).await;
            return Err(Status::deadline_exceeded("provider-local deadline expired"));
        }
        let removed = self
            .state
            .handles
            .lock()
            .await
            .remove(&fence.load_handle_id);
        let already = self
            .state
            .unloaded
            .lock()
            .await
            .contains(&fence.load_handle_id);
        if removed {
            self.state
                .handle_models
                .lock()
                .await
                .remove(&fence.load_handle_id);
            self.state
                .unloaded
                .lock()
                .await
                .insert(fence.load_handle_id.clone());
        }
        self.state.unload_calls.fetch_add(1, Ordering::AcqRel);
        self.finish_call(&fence.call_id).await;
        Ok(Response::new(wire::UnloadModelResponse {
            fence: Some(self.response_fence(fence)),
            outcome: if self
                .state
                .force_already_unloaded
                .swap(false, Ordering::AcqRel)
            {
                UnloadOutcome::AlreadyUnloaded as i32
            } else if removed {
                UnloadOutcome::Unloaded as i32
            } else if already {
                UnloadOutcome::AlreadyUnloaded as i32
            } else {
                UnloadOutcome::Unspecified as i32
            },
            error: None,
        }))
    }

    async fn infer(
        &self,
        request: Request<wire::InferRequest>,
    ) -> Result<Response<wire::InferResponse>, Status> {
        let request = request.into_inner();
        let deadline = Self::call_deadline(request.timeout_ms)?;
        let fence = self.validate_fence(request.fence.as_ref()).await?;
        self.acquire_handle(&fence.load_handle_id).await?;
        let cancel = match self.begin_call(&fence).await {
            Ok(cancel) => cancel,
            Err(error) => {
                self.state
                    .busy_handles
                    .lock()
                    .await
                    .remove(&fence.load_handle_id);
                return Err(error);
            }
        };
        self.state.infer_calls.fetch_add(1, Ordering::AcqRel);
        self.state.call_started.notify_waiters();
        if self.state.block_infer.load(Ordering::Acquire)
            && let Err(error) = self.wait_for_blocked_work(&cancel, deadline).await
        {
            self.finish_call(&fence.call_id).await;
            self.state
                .busy_handles
                .lock()
                .await
                .remove(&fence.load_handle_id);
            return Err(error);
        }
        self.finish_call(&fence.call_id).await;
        self.state
            .busy_handles
            .lock()
            .await
            .remove(&fence.load_handle_id);
        Ok(Response::new(wire::InferResponse {
            fence: Some(self.response_fence(fence)),
            outcome: if cancel.is_cancelled() {
                CallOutcome::Cancelled as i32
            } else {
                CallOutcome::Succeeded as i32
            },
            tensor_json: if self.state.oversized_response.load(Ordering::Acquire) {
                vec![b'x'; 129]
            } else {
                RESULT.to_vec()
            },
            error: None,
        }))
    }

    async fn health(
        &self,
        request: Request<wire::HealthRequest>,
    ) -> Result<Response<wire::HealthResponse>, Status> {
        let request = request.into_inner();
        let deadline = Self::call_deadline(request.timeout_ms)?;
        let fence = self.validate_fence(request.fence.as_ref()).await?;
        self.acquire_handle(&fence.load_handle_id).await?;
        let cancel = match self.begin_call(&fence).await {
            Ok(cancel) => cancel,
            Err(error) => {
                self.state
                    .busy_handles
                    .lock()
                    .await
                    .remove(&fence.load_handle_id);
                return Err(error);
            }
        };
        self.state.call_started.notify_waiters();
        if self.state.block_health.load(Ordering::Acquire) {
            if let Err(error) = self.wait_for_blocked_work(&cancel, deadline).await {
                self.finish_call(&fence.call_id).await;
                self.state
                    .busy_handles
                    .lock()
                    .await
                    .remove(&fence.load_handle_id);
                return Err(error);
            }
            if cancel.is_cancelled() {
                self.finish_call(&fence.call_id).await;
                self.state
                    .busy_handles
                    .lock()
                    .await
                    .remove(&fence.load_handle_id);
                return Err(Status::cancelled("provider call cancelled"));
            }
        }
        if base::tokio::time::Instant::now() >= deadline {
            self.finish_call(&fence.call_id).await;
            self.state
                .busy_handles
                .lock()
                .await
                .remove(&fence.load_handle_id);
            return Err(Status::deadline_exceeded("provider-local deadline expired"));
        }
        self.state
            .busy_handles
            .lock()
            .await
            .remove(&fence.load_handle_id);
        let handle_model = self
            .state
            .handle_models
            .lock()
            .await
            .get(&fence.load_handle_id)
            .cloned();
        let unhealthy_model = if let Some(model) = handle_model {
            self.state.unhealthy_models.lock().await.contains(&model)
        } else {
            false
        };
        self.finish_call(&fence.call_id).await;
        Ok(Response::new(wire::HealthResponse {
            fence: Some(self.response_fence(fence)),
            outcome: if self.state.unhealthy.load(Ordering::Acquire) || unhealthy_model {
                HealthOutcome::Unhealthy as i32
            } else {
                HealthOutcome::Healthy as i32
            },
            error: None,
        }))
    }

    async fn cancel(
        &self,
        request: Request<wire::CancelRequest>,
    ) -> Result<Response<wire::CancelResponse>, Status> {
        let request = request.into_inner();
        let fence = self.validate_fence(request.fence.as_ref()).await?;
        let call = self
            .state
            .calls
            .lock()
            .await
            .get(&request.target_call_id)
            .map(|call| call.cancellation.clone());
        let outcome = if let Some(call) = call {
            call.cancel();
            self.state.cancel_seen.notify_waiters();
            CancelOutcome::Accepted
        } else {
            CancelOutcome::AlreadyTerminal
        };
        Ok(Response::new(wire::CancelResponse {
            fence: Some(self.response_fence(fence)),
            outcome: outcome as i32,
            error: None,
        }))
    }
}

struct TestServer {
    socket: PathBuf,
    state: Arc<FakeState>,
    shutdown: CancellationToken,
    join: base::tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(root: &TestRoot, name: &str, packages_root: PathBuf) -> Self {
        let socket = root.path().join(format!("{name}.sock"));
        Self::start_at(root, socket, format!("instance-{name}"), packages_root).await
    }

    async fn start_at(
        root: &TestRoot,
        socket: PathBuf,
        provider_instance_id: String,
        packages_root: PathBuf,
    ) -> Self {
        let owned = OwnedUdsListener::bind(&socket).await.unwrap();
        let expected_uid = std::fs::metadata(root.path()).unwrap().uid();
        let incoming = owned.incoming().map(move |result| {
            result.and_then(|stream| {
                if stream.peer_cred()?.uid() == expected_uid {
                    Ok(stream)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "fake provider rejected client UID",
                    ))
                }
            })
        });
        let state = Arc::new(FakeState::default());
        let service = FakeProvider {
            provider_instance_id,
            packages_root,
            state: state.clone(),
        };
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let join = base::tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(AvaiExternalRuntimeProviderServer::new(service))
                .serve_with_incoming_shutdown(incoming, stop.cancelled())
                .await
                .unwrap();
            owned.cleanup().unwrap();
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            base::tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Self {
            socket,
            state,
            shutdown,
            join,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.join.await.unwrap();
        assert!(!self.socket.exists());
    }

    async fn crash(self) {
        self.join.abort();
        let _ = self.join.await;
        if self.socket.exists() {
            std::fs::remove_file(&self.socket).unwrap();
        }
    }
}

fn validate_load_artifact(root: &Path, request: &wire::LoadModelRequest) -> Result<(), Status> {
    let model = request
        .model
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("missing model identity"))?;
    for value in [&model.model_id, &model.version, &model.revision] {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':')
            })
        {
            return Err(Status::invalid_argument("unsafe model identity"));
        }
    }
    let relative = Path::new(&request.artifact_relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Status::invalid_argument("unconfined artifact path"));
    }
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|_| Status::invalid_argument("packages root missing"))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(Status::invalid_argument("unsafe packages root"));
    }
    let mut components = vec![
        model.model_id.as_str(),
        model.version.as_str(),
        model.revision.as_str(),
    ];
    components.extend(relative.components().map(|part| match part {
        Component::Normal(value) => value.to_str().unwrap_or_default(),
        _ => unreachable!("relative path was validated above"),
    }));
    let mut path = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| Status::invalid_argument("artifact component missing"))?;
        let final_component = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (!final_component && !metadata.is_dir())
            || (final_component && !metadata.is_file())
        {
            return Err(Status::invalid_argument("unsafe artifact component"));
        }
    }
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| Status::invalid_argument("artifact missing"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != request.artifact_size
    {
        return Err(Status::invalid_argument("artifact metadata mismatch"));
    }
    let bytes = std::fs::read(path).map_err(|_| Status::invalid_argument("artifact unreadable"))?;
    if !format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(&request.artifact_sha256) {
        return Err(Status::invalid_argument("artifact hash mismatch"));
    }
    if request.runtime_id != RUNTIME
        || request.runtime_contract_version != 1
        || request
            .execution
            .as_ref()
            .is_none_or(|execution| execution.version != 1)
    {
        return Err(Status::invalid_argument("execution contract mismatch"));
    }
    Ok(())
}

fn identity() -> ModelIdentity {
    ModelIdentity {
        model_id: "model-a".to_string(),
        version: "1".to_string(),
        revision: "rev-a".to_string(),
    }
}

fn package_policy() -> PackagePolicy {
    PackagePolicy {
        available_runtimes: HashSet::from([RUNTIME.to_string()]),
        allowed_result_schemas: HashSet::from([("gmv.tensor.outputs".to_string(), 1)]),
        approved_spdx: HashSet::from(["Apache-2.0".to_string()]),
        trusted_signing_keys: HashMap::from([(
            "test-key".to_string(),
            SigningKey::from_bytes(&[11; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        )]),
        max_memory_mb: 128,
        max_vram_mb: 0,
        ..PackagePolicy::default()
    }
}

fn write_package(root: &Path, model: &ModelIdentity) {
    let png = base::base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap();
    let files = [
        ("model/model.bin", b"external-model".as_slice()),
        (
            "schema/result.schema.json",
            br#"{"type":"object"}"#.as_slice(),
        ),
        ("tests/input.png", png.as_slice()),
        ("tests/expected.json", RESULT),
    ];
    for (relative, bytes) in files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let file_yaml = files
        .iter()
        .map(|(path, bytes)| {
            format!(
                "  - path: {path}\n    sha256: {:x}\n    size: {}",
                Sha256::digest(bytes),
                bytes.len()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let unsigned = format!(
        "api_version: gmv.ai/v1\nkind: ModelPlugin\nmetadata:\n  model_id: {}\n  version: '{}'\n  revision: {}\ncapabilities:\n  - {CAPABILITY}\nresult_schema:\n  name: gmv.tensor.outputs\n  version: 1\n  path: schema/result.schema.json\nvariants:\n  - runtime: {RUNTIME}\n    runtime_contract_version: 1\n    architecture: {}\n    accelerator: cpu\n    artifact: model/model.bin\nexecution:\n  version: 1\n  input:\n    kind: encoded_image_tensor_v1\n    accepted_media_types: [image/png]\n    max_bytes: 1024\n    max_width: 1\n    max_height: 1\n    tensor:\n      name: input\n      dtype: f32\n      layout: nchw\n      shape: [1, 3, 1, 1]\n    preprocess:\n      resize: exact\n      interpolation: bilinear\n      color: rgb\n      scale: 1.0\n      mean: [0.0, 0.0, 0.0]\n      std: [1.0, 1.0, 1.0]\n  outputs:\n    - name: output\n      dtype: f32\n      shape: [1, 3, 1, 1]\n  postprocess:\n    kind: tensor_json_v1\nresources:\n  memory_mb: 64\n  vram_mb: 0\n  max_batch: 1\nlicense:\n  spdx: Apache-2.0\n  commercial_use: true\n  redistribution: allowed\n  license_ref: ''\nself_test:\n  - input: tests/input.png\n    expected: tests/expected.json\n    oracle:\n      kind: json_numeric_v1\n      abs_tolerance: 0.0\n      rel_tolerance: 0.0\nfiles:\n{file_yaml}\nsigning:\n  key_id: test-key\n  signature: ''\n",
        model.model_id,
        model.version,
        model.revision,
        std::env::consts::ARCH,
    );
    let manifest: ModelPackageManifest = base::serde_yaml::from_str(&unsigned).unwrap();
    let signature =
        SigningKey::from_bytes(&[11; 32]).sign(&model_package_signing_payload(&manifest).unwrap());
    std::fs::write(
        root.join("manifest.yaml"),
        unsigned.replace(
            "signature: ''",
            &format!(
                "signature: {}",
                base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            ),
        ),
    )
    .unwrap();
}

async fn setup_repository(root: &TestRoot) -> ModelRepository {
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, &identity());
    let package = verify_package(&source, &package_policy()).unwrap();
    repository.install(&package, 1).await.unwrap();
    repository
}

async fn install_model(repository: &ModelRepository, root: &TestRoot, model: &ModelIdentity) {
    let source = root.path().join(format!("source-{}", model.revision));
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, model);
    let package = verify_package(&source, &package_policy()).unwrap();
    repository.install(&package, 1).await.unwrap();
}

fn config(root: &TestRoot, socket: PathBuf) -> ExternalProviderConfig {
    ExternalProviderConfig {
        socket_path: socket,
        expected_uid: std::fs::metadata(root.path()).unwrap().uid(),
        provider_id: PROVIDER_ID.to_string(),
        runtime_id: RUNTIME.to_string(),
        runtime_contract_version: 1,
        max_execution_calls: 4,
        max_input_bytes: 1024 * 1024,
        max_result_bytes: 1024 * 1024,
        connect_budget: Duration::from_secs(1),
        cancel_rpc_budget: Duration::from_millis(200),
        cancel_drain_grace: Duration::from_millis(300),
    }
}

async fn raw_client(socket: &Path) -> AvaiExternalRuntimeProviderClient<tonic::transport::Channel> {
    let channel = tonic::transport::Endpoint::try_from(format!("unix://{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    AvaiExternalRuntimeProviderClient::new(channel)
}

fn node_identity() -> NodeIdentity {
    NodeIdentity {
        node_id: "avai-external-test".to_string(),
        instance_id: "instance-test".to_string(),
        kind: NodeKind::Avai as i32,
    }
}

fn task_runtime(name: &str) -> GlobalRuntime {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-external-{name}-{id}"
    )))
    .unwrap()
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn task_request(root: &TestRoot, task_id: &str) -> CreateTaskRequest {
    let object_root = root.path().join("objects");
    std::fs::create_dir_all(&object_root).unwrap();
    let object_id = format!("object-{task_id}");
    let bytes = base::base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap();
    std::fs::write(object_root.join(&object_id), &bytes).unwrap();
    CreateTaskRequest {
        operation: Some(OperationRef {
            operation_id: format!("operation-{task_id}"),
            idempotency_key: format!("idempotency-{task_id}"),
        }),
        task_id: task_id.to_string(),
        capability: CAPABILITY.to_string(),
        expected_avai: Some(node_identity()),
        source: Some(SourceSpec {
            source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                owner: Some(node_identity()),
                resource: Some(ResourceRef {
                    resource_id: object_id.clone(),
                    resource_type: "avai_image".to_string(),
                }),
                metadata: Some(ImageMetadata {
                    content_type: "image/png".to_string(),
                    size_bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    width: 1,
                    height: 1,
                }),
                access: Some(AccessGrant {
                    grant_id: format!("grant-{task_id}"),
                    expected_consumer: Some(node_identity()),
                    purpose: CAPABILITY.to_string(),
                    expires_at_epoch_ms: now_epoch_ms() + 60_000,
                    endpoints: vec![DataEndpoint {
                        name: "image".to_string(),
                        uri: format!("gmv-object://{object_id}"),
                        ..Default::default()
                    }],
                    proof: vec![1],
                }),
            })),
        }),
        deadline_epoch_ms: 0,
        ..Default::default()
    }
}

async fn wait_terminal(
    tasks: &TaskManager,
    task_id: &str,
) -> gmv_protocol::avai::v1::QueryTaskResponse {
    for _ in 0..200 {
        let response = tasks
            .query_task(QueryTaskRequest {
                task_id: task_id.to_string(),
            })
            .await;
        if matches!(
            AiTaskState::try_from(response.state),
            Ok(AiTaskState::Succeeded | AiTaskState::Failed | AiTaskState::Cancelled)
        ) {
            return response;
        }
        base::tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task did not become terminal")
}

async fn preload_instance(
    repository: &ModelRepository,
    provider: &ExternalRuntimeProvider,
) -> Arc<dyn avai::model::ModelInstance> {
    let installed = repository.get(&identity()).await.unwrap().unwrap();
    provider
        .preload(
            &installed,
            RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn real_uds_handshake_enforces_socket_and_peer_credentials() {
    let root = TestRoot::new("security");
    let repository = setup_repository(&root).await;
    let packages = root.path().join("models/packages");
    let server = TestServer::start(&root, "security", packages).await;
    assert_eq!(
        std::fs::metadata(&server.socket)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let mut wrong_uid = config(&root, server.socket.clone());
    wrong_uid.expected_uid = wrong_uid.expected_uid.saturating_add(1);
    assert_eq!(
        ExternalRuntimeProvider::connect(wrong_uid)
            .await
            .err()
            .unwrap()
            .code,
        "model_runtime_provider_unavailable"
    );
    assert!(
        server.socket.exists(),
        "AVAI client must not unlink the socket"
    );
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn describe_rejects_wrong_provider_and_unsupported_version() {
    let root = TestRoot::new("describe-mismatch");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(
        &root,
        "describe-mismatch",
        root.path().join("models/packages"),
    )
    .await;
    let mut wrong_provider = config(&root, server.socket.clone());
    wrong_provider.provider_id = "wrong-provider".to_string();
    assert_eq!(
        ExternalRuntimeProvider::connect(wrong_provider)
            .await
            .err()
            .unwrap()
            .code,
        "model_runtime_protocol_mismatch"
    );
    server.state.wrong_protocol.store(true, Ordering::Release);
    assert_eq!(
        ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
            .await
            .err()
            .unwrap()
            .code,
        "model_runtime_protocol_mismatch"
    );
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn provider_independently_rejects_unconfined_or_changed_artifacts() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new("artifact-defense");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(
        &root,
        "artifact-defense",
        root.path().join("models/packages"),
    )
    .await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let _instance = preload_instance(&repository, &provider).await;
    let mut template = server.state.last_load.lock().await.clone().unwrap();
    let mut client = raw_client(&server.socket).await;
    let session = "raw-artifact-session".to_string();
    let describe = client
        .describe(wire::DescribeRequest {
            protocol_major: 1,
            min_minor: 0,
            max_minor: 0,
            provider_id: PROVIDER_ID.to_string(),
            client_session_id: session.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    let reset_fence = |request: &mut wire::LoadModelRequest, call: &str| {
        request.fence = Some(wire::Fence {
            provider_id: PROVIDER_ID.to_string(),
            provider_instance_id: describe.provider_instance_id.clone(),
            client_session_id: session.clone(),
            call_id: call.to_string(),
            load_handle_id: String::new(),
        });
    };
    reset_fence(&mut template, "load-valid");
    assert_eq!(
        client
            .load_model(template.clone())
            .await
            .unwrap()
            .into_inner()
            .outcome,
        LoadOutcome::Loaded as i32
    );
    for (call, path) in [
        ("load-absolute", "/tmp/model.bin"),
        ("load-traversal", "../model.bin"),
    ] {
        let mut request = template.clone();
        reset_fence(&mut request, call);
        request.artifact_relative_path = path.to_string();
        assert_eq!(
            client.load_model(request).await.unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
    }
    let mut wrong_hash = template.clone();
    reset_fence(&mut wrong_hash, "load-hash");
    wrong_hash.artifact_sha256 = "00".repeat(32);
    assert_eq!(
        client.load_model(wrong_hash).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let mut wrong_size = template.clone();
    reset_fence(&mut wrong_size, "load-size");
    wrong_size.artifact_size += 1;
    assert_eq!(
        client.load_model(wrong_size).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let installed = repository.get(&identity()).await.unwrap().unwrap();
    symlink("model.bin", installed.installed_path.join("model/link.bin")).unwrap();
    let mut symlink_request = template;
    reset_fence(&mut symlink_request, "load-symlink");
    symlink_request.artifact_relative_path = "model/link.bin".to_string();
    assert_eq!(
        client.load_model(symlink_request).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let outside = root.path().join("outside-artifacts");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("model.bin"), b"external-model").unwrap();
    std::fs::remove_dir_all(installed.installed_path.join("model")).unwrap();
    symlink(&outside, installed.installed_path.join("model")).unwrap();
    let mut intermediate_symlink = server.state.last_load.lock().await.clone().unwrap();
    reset_fence(&mut intermediate_symlink, "load-intermediate-symlink");
    assert_eq!(
        client
            .load_model(intermediate_symlink)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn provider_enforces_wire_deadline_without_cancel() {
    let root = TestRoot::new("provider-deadline");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(
        &root,
        "provider-deadline",
        root.path().join("models/packages"),
    )
    .await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let _instance = preload_instance(&repository, &provider).await;
    let mut request = server.state.last_load.lock().await.clone().unwrap();
    let mut client = raw_client(&server.socket).await;
    let session = "deadline-session".to_string();
    let describe = client
        .describe(wire::DescribeRequest {
            protocol_major: 1,
            min_minor: 0,
            max_minor: 0,
            provider_id: PROVIDER_ID.to_string(),
            client_session_id: session.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    let fence = |call_id: &str, load_handle_id: &str| wire::Fence {
        provider_id: PROVIDER_ID.to_string(),
        provider_instance_id: describe.provider_instance_id.clone(),
        client_session_id: session.clone(),
        call_id: call_id.to_string(),
        load_handle_id: load_handle_id.to_string(),
    };
    request.fence = Some(fence("provider-local-initial-load", ""));
    request.timeout_ms = 100;
    let handle = client
        .load_model(request.clone())
        .await
        .unwrap()
        .into_inner()
        .fence
        .unwrap()
        .load_handle_id;
    request.fence = Some(fence("provider-local-load-timeout", ""));
    request.timeout_ms = 30;
    server.state.block_load.store(true, Ordering::Release);
    assert_eq!(
        client.load_model(request).await.unwrap_err().code(),
        tonic::Code::DeadlineExceeded
    );
    server.state.block_load.store(false, Ordering::Release);
    server.state.block_infer.store(true, Ordering::Release);
    assert_eq!(
        client
            .infer(wire::InferRequest {
                fence: Some(fence("provider-local-infer-timeout", &handle)),
                timeout_ms: 30,
                input: Some(wire::RuntimeInput {
                    encoded: vec![1],
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                }),
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::DeadlineExceeded
    );
    server.state.block_infer.store(false, Ordering::Release);
    server.state.block_health.store(true, Ordering::Release);
    assert_eq!(
        client
            .health(wire::HealthRequest {
                fence: Some(fence("provider-local-health-timeout", &handle)),
                timeout_ms: 30,
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::DeadlineExceeded
    );
    server.state.block_health.store(false, Ordering::Release);
    server.state.block_unload.store(true, Ordering::Release);
    assert_eq!(
        client
            .unload_model(wire::UnloadModelRequest {
                fence: Some(fence("provider-local-unload-timeout", &handle)),
                timeout_ms: 30,
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::DeadlineExceeded
    );
    assert!(server.state.calls.lock().await.is_empty());
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn manager_load_self_test_infer_and_remote_unload_are_ordered() {
    let root = TestRoot::new("manager");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "manager", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    manager.preload(&identity(), 2).await.unwrap();
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Ready
    );
    server
        .state
        .force_already_unloaded
        .store(true, Ordering::Release);
    manager.unload(&identity(), 3).await.unwrap();
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Installed
    );
    assert_eq!(server.state.unload_calls.load(Ordering::Acquire), 1);
    manager.preload(&identity(), 4).await.unwrap();
    manager.activate(&identity(), 5).await.unwrap();
    let active = manager.capture(CAPABILITY).await.unwrap();
    let result = active
        .infer(
            RuntimeInput {
                encoded: vec![1, 2, 3].into(),
                media_type: "image/png".to_string(),
                width: 1,
                height: 1,
            },
            RuntimeCallContext::local(Duration::from_secs(1), CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(result.actual_model.model_id, "model-a");
    assert_eq!(result.actual_model.revision, "rev-a");
    assert_eq!(result.actual_model.runtime, RUNTIME);
    drop(active);
    assert_eq!(server.state.load_calls.load(Ordering::Acquire), 2);
    let load = server.state.last_load.lock().await.clone().unwrap();
    assert_eq!(load.artifact_relative_path, "model/model.bin");
    assert!(!Path::new(&load.artifact_relative_path).is_absolute());
    drop(manager);

    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let captured = manager.capture(CAPABILITY).await.unwrap();
    assert_eq!(captured.identity(), &identity());
    drop(captured);
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn task_manager_owned_image_uses_external_provider_and_local_actual_model() {
    let root = TestRoot::new("task-manager");
    let repository = setup_repository(&root).await;
    let server =
        TestServer::start(&root, "task-manager", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    manager.preload(&identity(), 2).await.unwrap();
    manager.activate(&identity(), 3).await.unwrap();
    let runtime = task_runtime("owned-image");
    let tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![CAPABILITY.to_string()],
        TaskManagerConfig {
            database_path: root.path().join("avai.db"),
            queue_size: 4,
            worker_count: 1,
            source_policy: SourcePolicy {
                object_root: root.path().join("objects"),
                ..SourcePolicy::default()
            },
            max_result_bytes: 1024 * 1024,
        },
        Some(manager),
        &runtime,
    )
    .await
    .unwrap();
    tasks
        .create_task(task_request(&root, "external-owned"), now_epoch_ms())
        .await;
    let terminal = wait_terminal(&tasks, "external-owned").await;
    assert_eq!(terminal.state, AiTaskState::Succeeded as i32);
    let actual = terminal.typed_result.unwrap().actual_model.unwrap();
    assert_eq!(actual.model_id, "model-a");
    assert_eq!(actual.version, "1");
    assert_eq!(actual.revision, "rev-a");
    assert_eq!(actual.runtime, RUNTIME);
    tasks.close_and_wait().await.unwrap();
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn startup_recovery_without_provider_fails_closed_and_preserves_active_truth() {
    let root = TestRoot::new("startup-absent");
    let repository = setup_repository(&root).await;
    let server =
        TestServer::start(&root, "startup-absent", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    manager.preload(&identity(), 2).await.unwrap();
    manager.activate(&identity(), 3).await.unwrap();
    drop(manager);
    server.stop().await;
    assert_eq!(
        ModelManager::open(
            repository.clone(),
            Vec::new(),
            ModelManagerConfig::default(),
        )
        .await
        .err()
        .unwrap()
        .code,
        "model_startup_recovery_failed"
    );
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Active
    );
    repository.close().await;
}

#[tokio::test]
async fn response_fence_independently_rejects_every_ephemeral_identity() {
    let root = TestRoot::new("fence");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "fence", root.path().join("models/packages")).await;
    for mismatch in 1..=4 {
        let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
            .await
            .unwrap();
        let instance = preload_instance(&repository, &provider).await;
        server.state.wrong_fence.store(mismatch, Ordering::Release);
        assert_eq!(
            instance
                .health(RuntimeCallContext::local(
                    Duration::from_secs(1),
                    CancellationToken::new(),
                ))
                .await
                .unwrap_err()
                .code,
            "model_runtime_stale_handle"
        );
        assert_eq!(
            instance
                .health(RuntimeCallContext::local(
                    Duration::from_secs(1),
                    CancellationToken::new(),
                ))
                .await
                .unwrap_err()
                .code,
            "model_runtime_stale_handle"
        );
    }
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn provider_boot_id_change_stales_old_handles_without_reload_or_truth_rewrite() {
    let root = TestRoot::new("provider-restart");
    let repository = setup_repository(&root).await;
    let packages = root.path().join("models/packages");
    let server = TestServer::start(&root, "provider-restart", packages.clone()).await;
    let socket = server.socket.clone();
    let provider = ExternalRuntimeProvider::connect(config(&root, socket.clone()))
        .await
        .unwrap();
    let old = preload_instance(&repository, &provider).await;
    server.crash().await;
    let restarted = TestServer::start_at(
        &root,
        socket.clone(),
        "instance-after-restart".to_string(),
        packages,
    )
    .await;
    assert!(matches!(
        old.health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap_err()
        .code,
        "model_runtime_provider_unavailable" | "model_runtime_stale_handle"
    ));
    assert_eq!(
        old.health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap_err()
        .code,
        "model_runtime_stale_handle"
    );
    let replacement = ExternalRuntimeProvider::connect(config(&root, socket))
        .await
        .unwrap();
    assert_eq!(restarted.state.load_calls.load(Ordering::Acquire), 0);
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Installed
    );
    let fresh = preload_instance(&repository, &replacement).await;
    fresh
        .health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    repository.close().await;
    restarted.stop().await;
}

#[tokio::test]
async fn replacement_session_stales_old_handle_without_auto_reload() {
    let root = TestRoot::new("session-replacement");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(
        &root,
        "session-replacement",
        root.path().join("models/packages"),
    )
    .await;
    let first = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let old = preload_instance(&repository, &first).await;
    let second = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    assert_eq!(
        old.health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap_err()
        .code,
        "model_runtime_stale_handle"
    );
    let fresh = preload_instance(&repository, &second).await;
    fresh
        .health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    assert_eq!(server.state.load_calls.load(Ordering::Acquire), 2);
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn replacement_session_is_busy_until_previous_work_is_terminal() {
    let root = TestRoot::new("session-drain");
    let repository = setup_repository(&root).await;
    let server =
        TestServer::start(&root, "session-drain", root.path().join("models/packages")).await;
    let first = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let old = preload_instance(&repository, &first).await;
    server.state.block_infer.store(true, Ordering::Release);
    server.state.ignore_cancel.store(true, Ordering::Release);
    let running = {
        let old = old.clone();
        base::tokio::spawn(async move {
            old.infer(
                RuntimeInput {
                    encoded: vec![1].into(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                },
                RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
            )
            .await
        })
    };
    server.state.call_started.notified().await;
    assert_eq!(
        ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
            .await
            .err()
            .unwrap()
            .code,
        "model_runtime_provider_unavailable"
    );
    assert_eq!(server.state.calls.lock().await.len(), 1);
    server.state.release.notify_waiters();
    assert_eq!(
        running.await.unwrap().unwrap_err().code,
        "model_runtime_cancelled"
    );
    let replacement = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let fresh = preload_instance(&repository, &replacement).await;
    fresh
        .health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn per_handle_lane_fences_health_and_unload_while_infer_is_admitted() {
    let root = TestRoot::new("handle-lane");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "handle-lane", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let instance = preload_instance(&repository, &provider).await;
    server.state.block_infer.store(true, Ordering::Release);
    let cancellation = CancellationToken::new();
    let call = {
        let instance = instance.clone();
        let cancellation = cancellation.clone();
        base::tokio::spawn(async move {
            instance
                .infer(
                    RuntimeInput {
                        encoded: vec![1].into(),
                        media_type: "image/png".to_string(),
                        width: 1,
                        height: 1,
                    },
                    RuntimeCallContext::local(Duration::from_secs(2), cancellation),
                )
                .await
        })
    };
    server.state.call_started.notified().await;
    assert_eq!(
        instance
            .health(RuntimeCallContext::local(
                Duration::from_secs(1),
                CancellationToken::new(),
            ))
            .await
            .unwrap_err()
            .code,
        "model_runtime_busy"
    );
    assert_eq!(
        instance
            .unload(RuntimeCallContext::local(
                Duration::from_secs(1),
                CancellationToken::new(),
            ))
            .await
            .unwrap_err()
            .code,
        "model_runtime_busy"
    );
    cancellation.cancel();
    assert_eq!(
        call.await.unwrap().unwrap_err().code,
        "model_runtime_cancelled"
    );
    instance
        .unload(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn global_execution_saturation_is_busy_while_cancel_control_remains_available() {
    let root = TestRoot::new("global-capacity");
    let repository = setup_repository(&root).await;
    let second = ModelIdentity {
        model_id: "model-b".to_string(),
        version: "1".to_string(),
        revision: "rev-b".to_string(),
    };
    install_model(&repository, &root, &second).await;
    let server = TestServer::start(
        &root,
        "global-capacity",
        root.path().join("models/packages"),
    )
    .await;
    let mut provider_config = config(&root, server.socket.clone());
    provider_config.max_execution_calls = 1;
    let provider = ExternalRuntimeProvider::connect(provider_config)
        .await
        .unwrap();
    let first_instance = preload_instance(&repository, &provider).await;
    let second_installed = repository.get(&second).await.unwrap().unwrap();
    let second_instance = provider
        .preload(
            &second_installed,
            RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
        )
        .await
        .unwrap();
    server.state.block_infer.store(true, Ordering::Release);
    let cancellation = CancellationToken::new();
    let running = {
        let first_instance = first_instance.clone();
        let cancellation = cancellation.clone();
        base::tokio::spawn(async move {
            first_instance
                .infer(
                    RuntimeInput {
                        encoded: vec![1].into(),
                        media_type: "image/png".to_string(),
                        width: 1,
                        height: 1,
                    },
                    RuntimeCallContext::local(Duration::from_secs(2), cancellation),
                )
                .await
        })
    };
    server.state.call_started.notified().await;
    assert_eq!(
        second_instance
            .health(RuntimeCallContext::local(
                Duration::from_secs(1),
                CancellationToken::new(),
            ))
            .await
            .unwrap_err()
            .code,
        "model_runtime_busy"
    );
    assert!(server.state.calls.lock().await.len() <= PROVIDER_MAX_CALLS);
    let cancel_seen = server.state.cancel_seen.notified();
    cancellation.cancel();
    base::tokio::time::timeout(Duration::from_secs(1), cancel_seen)
        .await
        .unwrap();
    assert_eq!(
        running.await.unwrap().unwrap_err().code,
        "model_runtime_cancelled"
    );
    assert!(server.state.calls.lock().await.is_empty());
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn oversized_request_and_response_fail_closed_without_truth_mutation() {
    let root = TestRoot::new("oversized");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "oversized", root.path().join("models/packages")).await;
    let mut provider_config = config(&root, server.socket.clone());
    provider_config.max_result_bytes = 128;
    let provider = ExternalRuntimeProvider::connect(provider_config)
        .await
        .unwrap();
    let instance = preload_instance(&repository, &provider).await;
    let calls_before = server.state.infer_calls.load(Ordering::Acquire);
    assert_eq!(
        instance
            .infer(
                RuntimeInput {
                    encoded: vec![0; 1024 * 1024 + 1].into(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                },
                RuntimeCallContext::local(Duration::from_secs(1), CancellationToken::new()),
            )
            .await
            .unwrap_err()
            .code,
        "model_runtime_contract_mismatch"
    );
    assert_eq!(
        server.state.infer_calls.load(Ordering::Acquire),
        calls_before
    );
    server
        .state
        .oversized_response
        .store(true, Ordering::Release);
    assert_eq!(
        instance
            .infer(
                RuntimeInput {
                    encoded: vec![1].into(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                },
                RuntimeCallContext::local(Duration::from_secs(1), CancellationToken::new()),
            )
            .await
            .unwrap_err()
            .code,
        "model_runtime_response_invalid"
    );
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Installed
    );
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn manager_cleans_remote_handle_after_health_rejection() {
    let root = TestRoot::new("cleanup-rejected");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(
        &root,
        "cleanup-rejected",
        root.path().join("models/packages"),
    )
    .await;
    server.state.unhealthy.store(true, Ordering::Release);
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        manager.preload(&identity(), 2).await.unwrap_err().code,
        "model_health_failed"
    );
    assert_eq!(server.state.unload_calls.load(Ordering::Acquire), 1);
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Failed
    );
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn external_health_failure_uses_existing_manager_fallback_authority() {
    let root = TestRoot::new("health-fallback");
    let repository = setup_repository(&root).await;
    let previous = identity();
    let active = ModelIdentity {
        model_id: "model-b".to_string(),
        version: "2".to_string(),
        revision: "rev-b".to_string(),
    };
    install_model(&repository, &root, &active).await;
    let server = TestServer::start(
        &root,
        "health-fallback",
        root.path().join("models/packages"),
    )
    .await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    manager.preload(&previous, 2).await.unwrap();
    manager.preload(&active, 3).await.unwrap();
    manager.activate(&previous, 4).await.unwrap();
    manager.activate(&active, 5).await.unwrap();
    server
        .state
        .unhealthy_models
        .lock()
        .await
        .insert(active.model_id.clone());
    assert!(matches!(
        manager.reconcile_active_health(CAPABILITY, 6).await.unwrap(),
        HealthReconcile::RolledBack { failed, .. } if failed == active
    ));
    let restored = manager.capture(CAPABILITY).await.unwrap();
    assert_eq!(restored.identity(), &previous);
    drop(restored);
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn provider_disconnect_fails_closed_without_rewriting_repository_truth() {
    let root = TestRoot::new("disconnect");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "disconnect", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let instance = preload_instance(&repository, &provider).await;
    server.stop().await;
    assert_eq!(
        instance
            .health(RuntimeCallContext::local(
                Duration::from_secs(1),
                CancellationToken::new(),
            ))
            .await
            .unwrap_err()
            .code,
        "model_runtime_provider_unavailable"
    );
    assert_eq!(
        repository.get(&identity()).await.unwrap().unwrap().state,
        ModelState::Installed
    );
    repository.close().await;
}

#[tokio::test]
async fn disconnect_during_each_execution_rpc_never_invents_success() {
    let load_root = TestRoot::new("disconnect-load");
    let load_repository = setup_repository(&load_root).await;
    let load_server = TestServer::start(
        &load_root,
        "disconnect-load",
        load_root.path().join("models/packages"),
    )
    .await;
    let load_state = load_server.state.clone();
    load_state.block_load.store(true, Ordering::Release);
    let load_provider =
        ExternalRuntimeProvider::connect(config(&load_root, load_server.socket.clone()))
            .await
            .unwrap();
    let load_manager = ModelManager::open(
        load_repository.clone(),
        vec![Arc::new(load_provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let load_manager_call = load_manager.clone();
    let load = base::tokio::spawn(async move {
        load_manager_call
            .preload_with_context(
                &identity(),
                2,
                RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
            )
            .await
    });
    load_state.call_started.notified().await;
    load_server.crash().await;
    assert_eq!(
        load.await.unwrap().err().unwrap().code,
        "model_runtime_provider_unavailable"
    );
    assert!(!matches!(
        load_repository
            .get(&identity())
            .await
            .unwrap()
            .unwrap()
            .state,
        ModelState::Ready | ModelState::Active
    ));
    drop(load_manager);
    load_repository.close().await;

    let infer_root = TestRoot::new("disconnect-infer");
    let infer_repository = setup_repository(&infer_root).await;
    let infer_server = TestServer::start(
        &infer_root,
        "disconnect-infer",
        infer_root.path().join("models/packages"),
    )
    .await;
    let infer_state = infer_server.state.clone();
    let infer_provider =
        ExternalRuntimeProvider::connect(config(&infer_root, infer_server.socket.clone()))
            .await
            .unwrap();
    let infer_instance = preload_instance(&infer_repository, &infer_provider).await;
    infer_state.block_infer.store(true, Ordering::Release);
    let infer = base::tokio::spawn(async move {
        infer_instance
            .infer(
                RuntimeInput {
                    encoded: vec![1].into(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                },
                RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
            )
            .await
    });
    infer_state.call_started.notified().await;
    infer_server.crash().await;
    assert_eq!(
        infer.await.unwrap().unwrap_err().code,
        "model_runtime_provider_unavailable"
    );
    assert_eq!(
        infer_repository
            .get(&identity())
            .await
            .unwrap()
            .unwrap()
            .state,
        ModelState::Installed
    );
    infer_repository.close().await;

    let health_root = TestRoot::new("disconnect-health");
    let health_repository = setup_repository(&health_root).await;
    let health_server = TestServer::start(
        &health_root,
        "disconnect-health",
        health_root.path().join("models/packages"),
    )
    .await;
    let health_state = health_server.state.clone();
    let health_provider =
        ExternalRuntimeProvider::connect(config(&health_root, health_server.socket.clone()))
            .await
            .unwrap();
    let health_instance = preload_instance(&health_repository, &health_provider).await;
    health_state.block_health.store(true, Ordering::Release);
    let health = base::tokio::spawn(async move {
        health_instance
            .health(RuntimeCallContext::local(
                Duration::from_secs(2),
                CancellationToken::new(),
            ))
            .await
    });
    health_state.call_started.notified().await;
    health_server.crash().await;
    assert_eq!(
        health.await.unwrap().unwrap_err().code,
        "model_runtime_provider_unavailable"
    );
    assert_eq!(
        health_repository
            .get(&identity())
            .await
            .unwrap()
            .unwrap()
            .state,
        ModelState::Installed
    );
    health_repository.close().await;

    let unload_root = TestRoot::new("disconnect-unload");
    let unload_repository = setup_repository(&unload_root).await;
    let unload_server = TestServer::start(
        &unload_root,
        "disconnect-unload",
        unload_root.path().join("models/packages"),
    )
    .await;
    let unload_state = unload_server.state.clone();
    let unload_provider =
        ExternalRuntimeProvider::connect(config(&unload_root, unload_server.socket.clone()))
            .await
            .unwrap();
    let unload_manager = ModelManager::open(
        unload_repository.clone(),
        vec![Arc::new(unload_provider) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    unload_manager.preload(&identity(), 2).await.unwrap();
    unload_state.block_unload.store(true, Ordering::Release);
    let unload_manager_call = unload_manager.clone();
    let unload = base::tokio::spawn(async move {
        unload_manager_call
            .unload_with_context(
                &identity(),
                3,
                RuntimeCallContext::local(Duration::from_secs(2), CancellationToken::new()),
            )
            .await
    });
    unload_state.call_started.notified().await;
    unload_server.crash().await;
    assert_eq!(
        unload.await.unwrap().unwrap_err().code,
        "model_runtime_provider_unavailable"
    );
    assert_eq!(
        unload_repository
            .get(&identity())
            .await
            .unwrap()
            .unwrap()
            .state,
        ModelState::Ready
    );
    drop(unload_manager);
    unload_repository.close().await;
}

#[tokio::test]
async fn cancel_ack_is_not_completion_and_original_call_is_drained() {
    let root = TestRoot::new("cancel-drain");
    let repository = setup_repository(&root).await;
    let server =
        TestServer::start(&root, "cancel-drain", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let instance = preload_instance(&repository, &provider).await;
    server.state.block_infer.store(true, Ordering::Release);
    server
        .state
        .hold_after_cancel
        .store(true, Ordering::Release);
    let cancellation = CancellationToken::new();
    let call = {
        let instance = instance.clone();
        let cancellation = cancellation.clone();
        base::tokio::spawn(async move {
            instance
                .infer(
                    RuntimeInput {
                        encoded: vec![1].into(),
                        media_type: "image/png".to_string(),
                        width: 1,
                        height: 1,
                    },
                    RuntimeCallContext::local(Duration::from_secs(2), cancellation),
                )
                .await
        })
    };
    server.state.call_started.notified().await;
    cancellation.cancel();
    server.state.cancel_seen.notified().await;
    base::tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !call.is_finished(),
        "Cancel ACCEPTED is not work completion"
    );
    server.state.release.notify_waiters();
    assert_eq!(
        call.await.unwrap().unwrap_err().code,
        "model_runtime_cancelled"
    );
    instance
        .health(RuntimeCallContext::local(
            Duration::from_secs(1),
            CancellationToken::new(),
        ))
        .await
        .unwrap();
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn cancelled_load_is_drained_without_leaving_a_reusable_handle() {
    let root = TestRoot::new("cancel-load");
    let repository = setup_repository(&root).await;
    let server = TestServer::start(&root, "cancel-load", root.path().join("models/packages")).await;
    let provider = ExternalRuntimeProvider::connect(config(&root, server.socket.clone()))
        .await
        .unwrap();
    let installed = repository.get(&identity()).await.unwrap().unwrap();
    server.state.block_load.store(true, Ordering::Release);
    let cancellation = CancellationToken::new();
    let call = {
        let provider = provider.clone();
        let cancellation = cancellation.clone();
        base::tokio::spawn(async move {
            provider
                .preload(
                    &installed,
                    RuntimeCallContext::local(Duration::from_secs(2), cancellation),
                )
                .await
        })
    };
    server.state.call_started.notified().await;
    cancellation.cancel();
    assert_eq!(
        call.await.unwrap().err().unwrap().code,
        "model_runtime_cancelled"
    );
    assert!(server.state.handles.lock().await.is_empty());
    repository.close().await;
    server.stop().await;
}

#[tokio::test]
async fn ignored_cancel_fences_session_but_preserves_deadline_cause() {
    let root = TestRoot::new("cancel-fence");
    let repository = setup_repository(&root).await;
    let server =
        TestServer::start(&root, "cancel-fence", root.path().join("models/packages")).await;
    let mut provider_config = config(&root, server.socket.clone());
    provider_config.cancel_drain_grace = Duration::from_millis(50);
    let provider = ExternalRuntimeProvider::connect(provider_config)
        .await
        .unwrap();
    let instance = preload_instance(&repository, &provider).await;
    server.state.block_infer.store(true, Ordering::Release);
    server.state.ignore_cancel.store(true, Ordering::Release);
    let cancellation = CancellationToken::new();
    let call = {
        let instance = instance.clone();
        let cancellation = cancellation.clone();
        base::tokio::spawn(async move {
            instance
                .infer(
                    RuntimeInput {
                        encoded: vec![1].into(),
                        media_type: "image/png".to_string(),
                        width: 1,
                        height: 1,
                    },
                    RuntimeCallContext::local(Duration::from_secs(2), cancellation),
                )
                .await
        })
    };
    server.state.call_started.notified().await;
    cancellation.cancel();
    assert_eq!(
        call.await.unwrap().unwrap_err().code,
        "model_runtime_cancelled"
    );
    assert_eq!(
        instance
            .health(RuntimeCallContext::local(
                Duration::from_secs(1),
                CancellationToken::new(),
            ))
            .await
            .unwrap_err()
            .code,
        "model_runtime_stale_handle"
    );
    server.state.release.notify_waiters();
    repository.close().await;
    server.stop().await;
}
