use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use base::tokio_util::sync::CancellationToken;
use gmv_protocol::component_management::v1::{
    AbortUpgradeRequest, ComponentAbortResponse, ComponentDrainResponse, DrainRequest,
    PrepareForUpgradeRequest,
    component_management_server::{ComponentManagement, ComponentManagementServer},
};
use tonic::{Request, Response, Status, async_trait};

use gmv_protocol::component_management::v1::{
    ComponentAbortOutcome, ComponentDrainOutcome, ComponentOwnerState,
};

#[async_trait]
pub trait ComponentDrainOwner: Send + Sync + 'static {
    async fn prepare_for_upgrade(
        &self,
        request: PrepareForUpgradeRequest,
    ) -> ComponentDrainResponse;

    async fn drain(&self, request: DrainRequest) -> ComponentDrainResponse;

    async fn abort_upgrade(&self, request: AbortUpgradeRequest) -> ComponentAbortResponse;
}

#[async_trait]
pub trait ComponentDrainBehavior: Send + Sync + 'static {
    fn supported(&self) -> bool;
    fn close_admission(&self);
    async fn reopen_admission(&self) -> Result<(), &'static str>;
    fn in_flight_admissions(&self) -> usize;
    async fn wait_for_admissions(&self);
    async fn is_drained(&self) -> bool;
    async fn drain_owned_resources(&self, cancel: CancellationToken) -> Result<(), &'static str>;
}

#[derive(Default)]
struct AdmissionState {
    accepting: bool,
    in_flight: usize,
}

struct AdmissionInner {
    state: Mutex<AdmissionState>,
    zero: base::tokio::sync::Notify,
}

#[derive(Clone)]
pub struct AdmissionBarrier {
    inner: Arc<AdmissionInner>,
}

impl Default for AdmissionBarrier {
    fn default() -> Self {
        Self {
            inner: Arc::new(AdmissionInner {
                state: Mutex::new(AdmissionState {
                    accepting: true,
                    in_flight: 0,
                }),
                zero: base::tokio::sync::Notify::new(),
            }),
        }
    }
}

impl AdmissionBarrier {
    pub fn acquire(&self) -> Option<AdmissionPermit> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return None;
        }
        state.in_flight += 1;
        Some(AdmissionPermit {
            inner: self.inner.clone(),
        })
    }

    pub fn close(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepting = false;
    }

    pub fn reopen(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepting = true;
    }

    pub fn in_flight(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .in_flight
    }

    pub async fn wait_for_zero(&self) {
        loop {
            let notified = self.inner.zero.notified();
            if self.in_flight() == 0 {
                return;
            }
            notified.await;
        }
    }
}

pub struct AdmissionPermit {
    inner: Arc<AdmissionInner>,
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.in_flight -= 1;
        if state.in_flight == 0 {
            self.inner.zero.notify_waiters();
        }
    }
}

#[derive(Debug, Clone)]
struct Generation {
    operation_id: String,
    drained: bool,
    failure: Option<&'static str>,
    cancel: CancellationToken,
    aborting: bool,
    abort_failure: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
struct OwnerState {
    generation: Option<Generation>,
    last_aborted_operation_id: Option<String>,
}

pub struct ManagedDrainOwner<B> {
    component_id: String,
    behavior: Arc<B>,
    state: Arc<base::tokio::sync::Mutex<OwnerState>>,
}

impl<B> ManagedDrainOwner<B> {
    pub fn new(component_id: impl Into<String>, behavior: Arc<B>) -> Self {
        Self {
            component_id: component_id.into(),
            behavior,
            state: Arc::new(base::tokio::sync::Mutex::new(OwnerState::default())),
        }
    }

    fn response(
        &self,
        operation_id: String,
        state: ComponentOwnerState,
        outcome: ComponentDrainOutcome,
        code: &str,
    ) -> ComponentDrainResponse {
        ComponentDrainResponse {
            operation_id,
            component_id: self.component_id.clone(),
            owner_state: state as i32,
            outcome: outcome as i32,
            stable_error_code: code.to_string(),
            observed_at_epoch_ms: now_ms(),
        }
    }

    fn validate(&self, operation_id: &str, component_id: &str) -> Option<ComponentDrainResponse> {
        if operation_id.is_empty() {
            return Some(self.response(
                operation_id.to_string(),
                ComponentOwnerState::Blocked,
                ComponentDrainOutcome::InternalFailure,
                "operation_id_missing",
            ));
        }
        if component_id != self.component_id {
            return Some(self.response(
                operation_id.to_string(),
                ComponentOwnerState::Blocked,
                ComponentDrainOutcome::OperationConflict,
                "component_mismatch",
            ));
        }
        None
    }

    fn abort_response(
        &self,
        operation_id: String,
        state: ComponentOwnerState,
        outcome: ComponentAbortOutcome,
        code: &str,
    ) -> ComponentAbortResponse {
        ComponentAbortResponse {
            operation_id,
            component_id: self.component_id.clone(),
            owner_state: state as i32,
            outcome: outcome as i32,
            stable_error_code: code.to_string(),
            observed_at_epoch_ms: now_ms(),
        }
    }

    fn validate_abort(
        &self,
        operation_id: &str,
        component_id: &str,
    ) -> Option<ComponentAbortResponse> {
        if operation_id.is_empty() {
            return Some(self.abort_response(
                operation_id.to_string(),
                ComponentOwnerState::Blocked,
                ComponentAbortOutcome::InternalFailure,
                "operation_id_missing",
            ));
        }
        if component_id != self.component_id {
            return Some(self.abort_response(
                operation_id.to_string(),
                ComponentOwnerState::Blocked,
                ComponentAbortOutcome::OperationConflict,
                "component_mismatch",
            ));
        }
        None
    }
}

#[async_trait]
impl<B: ComponentDrainBehavior> ComponentDrainOwner for ManagedDrainOwner<B> {
    async fn prepare_for_upgrade(
        &self,
        request: PrepareForUpgradeRequest,
    ) -> ComponentDrainResponse {
        if let Some(response) = self.validate(&request.operation_id, &request.component_id) {
            return response;
        }
        if !self.behavior.supported() {
            return self.response(
                request.operation_id,
                ComponentOwnerState::Accepting,
                ComponentDrainOutcome::Unsupported,
                "component_drain_unsupported",
            );
        }
        let mut state = self.state.lock().await;
        if let Some(current) = state.generation.as_ref() {
            if current.operation_id != request.operation_id {
                return self.response(
                    request.operation_id,
                    if current.drained {
                        ComponentOwnerState::Drained
                    } else {
                        ComponentOwnerState::Draining
                    },
                    ComponentDrainOutcome::OperationConflict,
                    "component_drain_operation_conflict",
                );
            }
            return self.response(
                request.operation_id,
                if current.failure.is_some() {
                    ComponentOwnerState::Blocked
                } else if current.drained {
                    ComponentOwnerState::Drained
                } else {
                    ComponentOwnerState::Draining
                },
                if current.failure.is_some() {
                    ComponentDrainOutcome::Busy
                } else if current.drained {
                    ComponentDrainOutcome::AlreadyDrained
                } else {
                    ComponentDrainOutcome::AlreadyDraining
                },
                current.failure.unwrap_or_default(),
            );
        }
        if state.last_aborted_operation_id.as_deref() == Some(request.operation_id.as_str()) {
            return self.response(
                request.operation_id,
                ComponentOwnerState::Accepting,
                ComponentDrainOutcome::Busy,
                "component_upgrade_aborted",
            );
        }
        if request.deadline_epoch_ms <= now_ms() {
            return self.response(
                request.operation_id,
                ComponentOwnerState::Accepting,
                ComponentDrainOutcome::DeadlineExceeded,
                "component_drain_deadline_exceeded",
            );
        }
        self.behavior.close_admission();
        let drained = self.behavior.in_flight_admissions() == 0 && self.behavior.is_drained().await;
        state.last_aborted_operation_id = None;
        let cancel = CancellationToken::new();
        state.generation = Some(Generation {
            operation_id: request.operation_id.clone(),
            drained,
            failure: None,
            cancel: cancel.clone(),
            aborting: false,
            abort_failure: None,
        });
        if !drained {
            let behavior = self.behavior.clone();
            let state = self.state.clone();
            let operation_id = request.operation_id.clone();
            base::tokio::spawn(async move {
                base::tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = behavior.wait_for_admissions() => {}
                }
                if cancel.is_cancelled() {
                    return;
                }
                let result = behavior.drain_owned_resources(cancel).await;
                let drained = behavior.in_flight_admissions() == 0 && behavior.is_drained().await;
                let mut state = state.lock().await;
                if let Some(current) = state.generation.as_mut()
                    && current.operation_id == operation_id
                {
                    current.drained = result.is_ok() && drained;
                    current.failure = match result {
                        Ok(()) if drained => None,
                        Ok(()) => Some("component_drain_incomplete"),
                        Err(code) => Some(code),
                    };
                }
            });
        }
        self.response(
            request.operation_id,
            if drained {
                ComponentOwnerState::Drained
            } else {
                ComponentOwnerState::Draining
            },
            if drained {
                ComponentDrainOutcome::Drained
            } else {
                ComponentDrainOutcome::Accepted
            },
            "",
        )
    }

    async fn drain(&self, request: DrainRequest) -> ComponentDrainResponse {
        if let Some(response) = self.validate(&request.operation_id, &request.component_id) {
            return response;
        }
        if !self.behavior.supported() {
            return self.response(
                request.operation_id,
                ComponentOwnerState::Accepting,
                ComponentDrainOutcome::Unsupported,
                "component_drain_unsupported",
            );
        }
        loop {
            let current = self.state.lock().await.generation.clone();
            let Some(current) = current else {
                return self.response(
                    request.operation_id,
                    ComponentOwnerState::Accepting,
                    ComponentDrainOutcome::Busy,
                    "component_drain_not_prepared",
                );
            };
            if current.operation_id != request.operation_id {
                return self.response(
                    request.operation_id,
                    if current.drained {
                        ComponentOwnerState::Drained
                    } else {
                        ComponentOwnerState::Draining
                    },
                    ComponentDrainOutcome::OperationConflict,
                    "component_drain_operation_conflict",
                );
            }
            if current.drained {
                return self.response(
                    request.operation_id,
                    ComponentOwnerState::Drained,
                    ComponentDrainOutcome::AlreadyDrained,
                    "",
                );
            }
            if let Some(code) = current.failure {
                return self.response(
                    request.operation_id,
                    ComponentOwnerState::Blocked,
                    if code == "component_drain_incomplete" {
                        ComponentDrainOutcome::Busy
                    } else {
                        ComponentDrainOutcome::InternalFailure
                    },
                    code,
                );
            }
            let remaining_ms = request.deadline_epoch_ms.saturating_sub(now_ms());
            if remaining_ms <= 0 {
                return self.response(
                    request.operation_id,
                    ComponentOwnerState::Draining,
                    ComponentDrainOutcome::DeadlineExceeded,
                    "component_drain_deadline_exceeded",
                );
            }
            base::tokio::time::sleep(Duration::from_millis((remaining_ms as u64).min(10))).await;
        }
    }

    async fn abort_upgrade(&self, request: AbortUpgradeRequest) -> ComponentAbortResponse {
        if let Some(response) = self.validate_abort(&request.operation_id, &request.component_id) {
            return response;
        }
        if !self.behavior.supported() {
            return self.abort_response(
                request.operation_id,
                ComponentOwnerState::Accepting,
                ComponentAbortOutcome::Unsupported,
                "component_abort_unsupported",
            );
        }
        let mut initiated = false;
        loop {
            let mut state = self.state.lock().await;
            let last_aborted_operation_id = state.last_aborted_operation_id.clone();
            match state.generation.as_mut() {
                Some(current) if current.operation_id != request.operation_id => {
                    return self.abort_response(
                        request.operation_id,
                        if current.drained {
                            ComponentOwnerState::Drained
                        } else {
                            ComponentOwnerState::Draining
                        },
                        ComponentAbortOutcome::OperationConflict,
                        "component_abort_operation_conflict",
                    );
                }
                Some(current) if initiated && current.abort_failure.is_some() => {
                    return self.abort_response(
                        request.operation_id,
                        ComponentOwnerState::Blocked,
                        ComponentAbortOutcome::InternalFailure,
                        current
                            .abort_failure
                            .unwrap_or("component_abort_internal_failure"),
                    );
                }
                Some(current) => {
                    if request.deadline_epoch_ms <= now_ms() {
                        return self.abort_response(
                            request.operation_id,
                            ComponentOwnerState::Draining,
                            ComponentAbortOutcome::DeadlineExceeded,
                            "component_abort_deadline_exceeded",
                        );
                    }
                    if !current.aborting {
                        current.cancel.cancel();
                        current.aborting = true;
                        current.abort_failure = None;
                        initiated = true;
                        let behavior = self.behavior.clone();
                        let owner_state = self.state.clone();
                        let operation_id = request.operation_id.clone();
                        base::tokio::spawn(async move {
                            let result = behavior.reopen_admission().await;
                            let mut state = owner_state.lock().await;
                            let Some(current) = state.generation.as_mut() else {
                                return;
                            };
                            if current.operation_id != operation_id || !current.aborting {
                                return;
                            }
                            match result {
                                Ok(()) => {
                                    state.generation = None;
                                    state.last_aborted_operation_id = Some(operation_id);
                                }
                                Err(code) => {
                                    current.aborting = false;
                                    current.abort_failure = Some(code);
                                }
                            }
                        });
                    }
                }
                None if last_aborted_operation_id.as_deref()
                    == Some(request.operation_id.as_str()) =>
                {
                    return self.abort_response(
                        request.operation_id,
                        ComponentOwnerState::Accepting,
                        if initiated {
                            ComponentAbortOutcome::Resumed
                        } else {
                            ComponentAbortOutcome::AlreadyAccepting
                        },
                        "",
                    );
                }
                None if last_aborted_operation_id.is_some() => {
                    return self.abort_response(
                        request.operation_id,
                        ComponentOwnerState::Accepting,
                        ComponentAbortOutcome::OperationConflict,
                        "component_abort_operation_conflict",
                    );
                }
                None => {
                    state.last_aborted_operation_id = Some(request.operation_id.clone());
                    return self.abort_response(
                        request.operation_id,
                        ComponentOwnerState::Accepting,
                        ComponentAbortOutcome::AlreadyAccepting,
                        "",
                    );
                }
            }
            drop(state);
            let remaining_ms = request.deadline_epoch_ms.saturating_sub(now_ms());
            if remaining_ms <= 0 {
                return self.abort_response(
                    request.operation_id,
                    ComponentOwnerState::Draining,
                    ComponentAbortOutcome::DeadlineExceeded,
                    "component_abort_deadline_exceeded",
                );
            }
            base::tokio::time::sleep(Duration::from_millis((remaining_ms as u64).min(10))).await;
        }
    }
}

#[derive(Debug)]
pub struct UnsupportedDrainOwner {
    component_id: String,
}

impl UnsupportedDrainOwner {
    pub fn new(component_id: impl Into<String>) -> Self {
        Self {
            component_id: component_id.into(),
        }
    }

    fn response(
        &self,
        operation_id: String,
        component_id: String,
        deadline_epoch_ms: i64,
    ) -> ComponentDrainResponse {
        let (outcome, code) = if component_id != self.component_id {
            (
                ComponentDrainOutcome::OperationConflict,
                "component_mismatch",
            )
        } else if deadline_epoch_ms <= now_ms() {
            (
                ComponentDrainOutcome::DeadlineExceeded,
                "component_drain_deadline_exceeded",
            )
        } else {
            (
                ComponentDrainOutcome::Unsupported,
                "component_drain_unsupported",
            )
        };
        ComponentDrainResponse {
            operation_id,
            component_id: self.component_id.clone(),
            owner_state: ComponentOwnerState::Accepting as i32,
            outcome: outcome as i32,
            stable_error_code: code.to_string(),
            observed_at_epoch_ms: now_ms(),
        }
    }
}

#[async_trait]
impl ComponentDrainOwner for UnsupportedDrainOwner {
    async fn prepare_for_upgrade(
        &self,
        request: PrepareForUpgradeRequest,
    ) -> ComponentDrainResponse {
        self.response(
            request.operation_id,
            request.component_id,
            request.deadline_epoch_ms,
        )
    }

    async fn drain(&self, request: DrainRequest) -> ComponentDrainResponse {
        self.response(
            request.operation_id,
            request.component_id,
            request.deadline_epoch_ms,
        )
    }

    async fn abort_upgrade(&self, request: AbortUpgradeRequest) -> ComponentAbortResponse {
        let (outcome, code) = if request.component_id != self.component_id {
            (
                ComponentAbortOutcome::OperationConflict,
                "component_mismatch",
            )
        } else if request.deadline_epoch_ms <= now_ms() {
            (
                ComponentAbortOutcome::DeadlineExceeded,
                "component_abort_deadline_exceeded",
            )
        } else {
            (
                ComponentAbortOutcome::Unsupported,
                "component_abort_unsupported",
            )
        };
        ComponentAbortResponse {
            operation_id: request.operation_id,
            component_id: self.component_id.clone(),
            owner_state: ComponentOwnerState::Accepting as i32,
            outcome: outcome as i32,
            stable_error_code: code.to_string(),
            observed_at_epoch_ms: now_ms(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[derive(Clone)]
pub struct ComponentManagementRpc<O> {
    owner: Arc<O>,
}

impl<O> ComponentManagementRpc<O> {
    pub fn new(owner: Arc<O>) -> Self {
        Self { owner }
    }
}

#[async_trait]
impl<O: ComponentDrainOwner> ComponentManagement for ComponentManagementRpc<O> {
    async fn prepare_for_upgrade(
        &self,
        request: Request<PrepareForUpgradeRequest>,
    ) -> Result<Response<ComponentDrainResponse>, Status> {
        Ok(Response::new(
            self.owner.prepare_for_upgrade(request.into_inner()).await,
        ))
    }

    async fn drain(
        &self,
        request: Request<DrainRequest>,
    ) -> Result<Response<ComponentDrainResponse>, Status> {
        Ok(Response::new(self.owner.drain(request.into_inner()).await))
    }

    async fn abort_upgrade(
        &self,
        request: Request<AbortUpgradeRequest>,
    ) -> Result<Response<ComponentAbortResponse>, Status> {
        Ok(Response::new(
            self.owner.abort_upgrade(request.into_inner()).await,
        ))
    }
}

#[cfg(unix)]
pub async fn serve_uds<O: ComponentDrainOwner>(
    socket: &Path,
    owner: Arc<O>,
    cancel: CancellationToken,
) -> base::exception::GlobalResult<()> {
    use base::futures::stream;

    if !socket.is_absolute()
        || socket.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(base::exception::GlobalError::new_sys_error(
            "component management socket must be an absolute normalized path",
            |_| {},
        ));
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| base::exception::GlobalError::from_external_error(error, |_| {}))?;
    }
    let listener = bind_uds_listener(socket).await?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| base::exception::GlobalError::from_external_error(error, |_| {}))?;
    let owned_socket = socket_identity(socket)?;
    let incoming = stream::unfold(listener, |listener| async move {
        Some((listener.accept().await.map(|(stream, _)| stream), listener))
    });
    let result = tonic::transport::Server::builder()
        .add_service(ComponentManagementServer::new(ComponentManagementRpc::new(
            owner,
        )))
        .serve_with_incoming_shutdown(incoming, async move { cancel.cancelled().await })
        .await;
    remove_owned_socket(socket, owned_socket)?;
    result.map_err(|error| base::exception::GlobalError::from_external_error(error, |_| {}))
}

#[cfg(unix)]
async fn bind_uds_listener(
    socket: &Path,
) -> base::exception::GlobalResult<base::tokio::net::UnixListener> {
    match base::tokio::net::UnixListener::bind(socket) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            let stale_identity = socket_identity(socket)?;
            match base::tokio::net::UnixStream::connect(socket).await {
                Ok(_) => Err(io_error(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "component management socket is active",
                ))),
                Err(connect_error)
                    if connect_error.kind() == std::io::ErrorKind::ConnectionRefused =>
                {
                    if socket_identity(socket)? != stale_identity {
                        return Err(io_error(std::io::Error::new(
                            std::io::ErrorKind::AddrInUse,
                            "component management socket changed during stale recovery",
                        )));
                    }
                    std::fs::remove_file(socket).map_err(io_error)?;
                    base::tokio::net::UnixListener::bind(socket).map_err(io_error)
                }
                Err(connect_error) => Err(io_error(connect_error)),
            }
        }
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(unix)]
fn socket_identity(socket: &Path) -> base::exception::GlobalResult<(u64, u64)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = std::fs::symlink_metadata(socket).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(io_error(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "component management path is not a socket",
        )));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
fn remove_owned_socket(
    socket: &Path,
    owned_identity: (u64, u64),
) -> base::exception::GlobalResult<()> {
    match std::fs::symlink_metadata(socket) {
        Ok(_) if socket_identity(socket)? == owned_identity => {
            std::fs::remove_file(socket).map_err(io_error)
        }
        Ok(_) => Err(io_error(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "component management socket ownership changed before cleanup",
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(unix)]
fn io_error(error: std::io::Error) -> base::exception::GlobalError {
    base::exception::GlobalError::from_external_error(error, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TestBehavior {
        admission: AdmissionBarrier,
        drained: AtomicBool,
        release: Option<Arc<base::tokio::sync::Notify>>,
        drain_calls: AtomicUsize,
    }

    struct SlowResumeBehavior {
        admission: AdmissionBarrier,
        reopen_started: Arc<base::tokio::sync::Semaphore>,
        reopen_release: Arc<base::tokio::sync::Notify>,
    }

    #[async_trait]
    impl ComponentDrainBehavior for SlowResumeBehavior {
        fn supported(&self) -> bool {
            true
        }
        fn close_admission(&self) {
            self.admission.close();
        }
        async fn reopen_admission(&self) -> Result<(), &'static str> {
            self.reopen_started.add_permits(1);
            self.reopen_release.notified().await;
            self.admission.reopen();
            Ok(())
        }
        fn in_flight_admissions(&self) -> usize {
            self.admission.in_flight()
        }
        async fn wait_for_admissions(&self) {
            self.admission.wait_for_zero().await;
        }
        async fn is_drained(&self) -> bool {
            false
        }
        async fn drain_owned_resources(
            &self,
            cancel: CancellationToken,
        ) -> Result<(), &'static str> {
            cancel.cancelled().await;
            Ok(())
        }
    }

    #[async_trait]
    impl ComponentDrainBehavior for TestBehavior {
        fn supported(&self) -> bool {
            true
        }
        fn close_admission(&self) {
            self.admission.close();
        }
        async fn reopen_admission(&self) -> Result<(), &'static str> {
            self.admission.reopen();
            Ok(())
        }
        fn in_flight_admissions(&self) -> usize {
            self.admission.in_flight()
        }
        async fn wait_for_admissions(&self) {
            self.admission.wait_for_zero().await;
        }
        async fn is_drained(&self) -> bool {
            self.drained.load(Ordering::Acquire)
        }
        async fn drain_owned_resources(
            &self,
            cancel: CancellationToken,
        ) -> Result<(), &'static str> {
            self.drain_calls.fetch_add(1, Ordering::AcqRel);
            if let Some(release) = &self.release {
                base::tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = release.notified() => {}
                }
            }
            self.drained.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn future_deadline() -> i64 {
        now_ms() + 10_000
    }

    #[tokio::test]
    async fn replay_keeps_generation_and_conflicting_operation_is_rejected() {
        let release = Arc::new(base::tokio::sync::Notify::new());
        let behavior = Arc::new(TestBehavior {
            admission: AdmissionBarrier::default(),
            drained: AtomicBool::new(false),
            release: Some(release.clone()),
            drain_calls: AtomicUsize::new(0),
        });
        let owner = ManagedDrainOwner::new("stream", behavior.clone());
        let request = PrepareForUpgradeRequest {
            operation_id: "op-1".into(),
            component_id: "stream".into(),
            deadline_epoch_ms: future_deadline(),
        };
        let first = owner.prepare_for_upgrade(request.clone()).await;
        let replay = owner.prepare_for_upgrade(request).await;
        let expired_replay = owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: now_ms() - 1,
            })
            .await;
        let conflict = owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-2".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let cross_target = owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "session".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(first.outcome, ComponentDrainOutcome::Accepted as i32);
        assert_eq!(
            replay.outcome,
            ComponentDrainOutcome::AlreadyDraining as i32
        );
        assert_eq!(
            expired_replay.outcome,
            ComponentDrainOutcome::AlreadyDraining as i32
        );
        assert_eq!(
            conflict.outcome,
            ComponentDrainOutcome::OperationConflict as i32
        );
        assert_eq!(
            cross_target.outcome,
            ComponentDrainOutcome::OperationConflict as i32
        );
        assert!(behavior.admission.acquire().is_none());
        release.notify_one();
        let first_drain = owner
            .drain(DrainRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let replay_drain = owner
            .drain(DrainRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(
            first_drain.outcome,
            ComponentDrainOutcome::AlreadyDrained as i32
        );
        assert_eq!(
            replay_drain.outcome,
            ComponentDrainOutcome::AlreadyDrained as i32
        );
        assert_eq!(behavior.drain_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn deadline_does_not_reopen_admission() {
        let behavior = Arc::new(TestBehavior {
            admission: AdmissionBarrier::default(),
            drained: AtomicBool::new(false),
            release: Some(Arc::new(base::tokio::sync::Notify::new())),
            drain_calls: AtomicUsize::new(0),
        });
        let owner = ManagedDrainOwner::new("avai", behavior.clone());
        owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "avai".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let response = owner
            .drain(DrainRequest {
                operation_id: "op-1".into(),
                component_id: "avai".into(),
                deadline_epoch_ms: now_ms() + 10,
            })
            .await;
        assert_eq!(response.owner_state, ComponentOwnerState::Draining as i32);
        assert_eq!(
            response.outcome,
            ComponentDrainOutcome::DeadlineExceeded as i32
        );
        assert!(behavior.admission.acquire().is_none());
    }

    #[tokio::test]
    async fn abort_reopens_admission_and_is_idempotent_and_fenced() {
        let behavior = Arc::new(TestBehavior {
            admission: AdmissionBarrier::default(),
            drained: AtomicBool::new(false),
            release: Some(Arc::new(base::tokio::sync::Notify::new())),
            drain_calls: AtomicUsize::new(0),
        });
        let owner = ManagedDrainOwner::new("stream", behavior.clone());
        owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let aborted = owner
            .abort_upgrade(AbortUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(aborted.owner_state, ComponentOwnerState::Accepting as i32);
        assert_eq!(aborted.outcome, ComponentAbortOutcome::Resumed as i32);
        assert!(behavior.admission.acquire().is_some());

        let replay = owner
            .abort_upgrade(AbortUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: now_ms() - 1,
            })
            .await;
        assert_eq!(
            replay.outcome,
            ComponentAbortOutcome::AlreadyAccepting as i32
        );
        let conflicting_abort = owner
            .abort_upgrade(AbortUpgradeRequest {
                operation_id: "op-2".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(
            conflicting_abort.outcome,
            ComponentAbortOutcome::OperationConflict as i32
        );
        let late_prepare = owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(
            late_prepare.owner_state,
            ComponentOwnerState::Accepting as i32
        );
        assert_eq!(late_prepare.outcome, ComponentDrainOutcome::Busy as i32);
        assert!(behavior.admission.acquire().is_some());

        owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-2".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let late_abort = owner
            .abort_upgrade(AbortUpgradeRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(
            late_abort.outcome,
            ComponentAbortOutcome::OperationConflict as i32
        );
    }

    #[tokio::test]
    async fn abort_on_fresh_owner_confirms_accepting_and_replays() {
        let behavior = Arc::new(TestBehavior {
            admission: AdmissionBarrier::default(),
            drained: AtomicBool::new(false),
            release: None,
            drain_calls: AtomicUsize::new(0),
        });
        let owner = ManagedDrainOwner::new("avai", behavior.clone());
        for deadline_epoch_ms in [future_deadline(), now_ms() - 1] {
            let response = owner
                .abort_upgrade(AbortUpgradeRequest {
                    operation_id: "op-uncertain".into(),
                    component_id: "avai".into(),
                    deadline_epoch_ms,
                })
                .await;
            assert_eq!(response.owner_state, ComponentOwnerState::Accepting as i32);
            assert_eq!(
                response.outcome,
                ComponentAbortOutcome::AlreadyAccepting as i32
            );
        }
        assert!(behavior.admission.acquire().is_some());
    }

    #[tokio::test]
    async fn dropped_abort_caller_does_not_cancel_resume_and_replay_observes_truth() {
        let behavior = Arc::new(SlowResumeBehavior {
            admission: AdmissionBarrier::default(),
            reopen_started: Arc::new(base::tokio::sync::Semaphore::new(0)),
            reopen_release: Arc::new(base::tokio::sync::Notify::new()),
        });
        let owner = Arc::new(ManagedDrainOwner::new("stream", behavior.clone()));
        owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-loss".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        let abort_owner = owner.clone();
        let caller = base::tokio::spawn(async move {
            abort_owner
                .abort_upgrade(AbortUpgradeRequest {
                    operation_id: "op-loss".into(),
                    component_id: "stream".into(),
                    deadline_epoch_ms: future_deadline(),
                })
                .await
        });
        behavior.reopen_started.acquire().await.unwrap().forget();
        caller.abort();
        behavior.reopen_release.notify_one();
        let replay = owner
            .abort_upgrade(AbortUpgradeRequest {
                operation_id: "op-loss".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(replay.owner_state, ComponentOwnerState::Accepting as i32);
        assert_eq!(
            replay.outcome,
            ComponentAbortOutcome::AlreadyAccepting as i32
        );
        assert!(behavior.admission.acquire().is_some());
    }

    #[tokio::test]
    async fn unsupported_busy_and_component_route_isolation_are_typed() {
        for component_id in ["guard", "session", "stream", "avai"] {
            let unsupported = UnsupportedDrainOwner::new(component_id);
            let response = unsupported
                .prepare_for_upgrade(PrepareForUpgradeRequest {
                    operation_id: "op-1".into(),
                    component_id: component_id.into(),
                    deadline_epoch_ms: future_deadline(),
                })
                .await;
            assert_eq!(response.outcome, ComponentDrainOutcome::Unsupported as i32);
            let abort = unsupported
                .abort_upgrade(AbortUpgradeRequest {
                    operation_id: "op-1".into(),
                    component_id: component_id.into(),
                    deadline_epoch_ms: future_deadline(),
                })
                .await;
            assert_eq!(abort.outcome, ComponentAbortOutcome::Unsupported as i32);
            let cross_target = unsupported
                .prepare_for_upgrade(PrepareForUpgradeRequest {
                    operation_id: "op-1".into(),
                    component_id: "other".into(),
                    deadline_epoch_ms: future_deadline(),
                })
                .await;
            assert_eq!(
                cross_target.outcome,
                ComponentDrainOutcome::OperationConflict as i32
            );
        }

        let owner = ManagedDrainOwner::new(
            "stream",
            Arc::new(TestBehavior {
                admission: AdmissionBarrier::default(),
                drained: AtomicBool::new(false),
                release: None,
                drain_calls: AtomicUsize::new(0),
            }),
        );
        let busy = owner
            .drain(DrainRequest {
                operation_id: "op-1".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(busy.owner_state, ComponentOwnerState::Accepting as i32);
        assert_eq!(busy.outcome, ComponentDrainOutcome::Busy as i32);
    }

    #[tokio::test]
    async fn admitted_request_blocks_terminal_drain_until_permit_drops() {
        let behavior = Arc::new(TestBehavior {
            admission: AdmissionBarrier::default(),
            drained: AtomicBool::new(true),
            release: None,
            drain_calls: AtomicUsize::new(0),
        });
        let permit = behavior.admission.acquire().unwrap();
        let owner = Arc::new(ManagedDrainOwner::new("stream", behavior.clone()));
        let prepared = owner
            .prepare_for_upgrade(PrepareForUpgradeRequest {
                operation_id: "op-permit".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(prepared.owner_state, ComponentOwnerState::Draining as i32);
        assert_eq!(prepared.outcome, ComponentDrainOutcome::Accepted as i32);
        assert!(behavior.admission.acquire().is_none());

        drop(permit);
        let drained = owner
            .drain(DrainRequest {
                operation_id: "op-permit".into(),
                component_id: "stream".into(),
                deadline_epoch_ms: future_deadline(),
            })
            .await;
        assert_eq!(drained.owner_state, ComponentOwnerState::Drained as i32);
        assert_eq!(behavior.admission.in_flight(), 0);
        assert!(behavior.admission.acquire().is_none());
    }

    #[cfg(unix)]
    fn socket_test_root(name: &str) -> std::path::PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gmv-component-management-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[cfg(unix)]
    async fn wait_for_path(path: &Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            base::tokio::task::yield_now().await;
        }
        panic!("socket path was not created: {}", path.display());
    }

    #[cfg(unix)]
    async fn wait_for_socket(path: &Path) {
        for _ in 0..100 {
            if base::tokio::net::UnixStream::connect(path).await.is_ok() {
                return;
            }
            base::tokio::task::yield_now().await;
        }
        panic!("socket did not accept connections: {}", path.display());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uds_clean_bind_and_normal_shutdown_remove_owned_socket() {
        let root = socket_test_root("clean");
        let socket = root.join("management.sock");
        let cancel = CancellationToken::new();
        let server_socket = socket.clone();
        let server_cancel = cancel.clone();
        let server = base::tokio::spawn(async move {
            serve_uds(
                &server_socket,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                server_cancel,
            )
            .await
        });
        wait_for_socket(&socket).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert!(!socket.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uds_recovers_only_a_stale_socket() {
        let root = socket_test_root("stale");
        let socket = root.join("management.sock");
        let stale = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        drop(stale);
        let cancel = CancellationToken::new();
        let server_socket = socket.clone();
        let server_cancel = cancel.clone();
        let server = base::tokio::spawn(async move {
            serve_uds(
                &server_socket,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                server_cancel,
            )
            .await
        });
        wait_for_socket(&socket).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uds_active_socket_is_not_replaced() {
        let root = socket_test_root("active");
        let socket = root.join("management.sock");
        let first_cancel = CancellationToken::new();
        let first_socket = socket.clone();
        let first_server_cancel = first_cancel.clone();
        let first = base::tokio::spawn(async move {
            serve_uds(
                &first_socket,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                first_server_cancel,
            )
            .await
        });
        wait_for_path(&socket).await;
        let identity = socket_identity(&socket).unwrap();
        assert!(
            serve_uds(
                &socket,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                CancellationToken::new(),
            )
            .await
            .is_err()
        );
        assert_eq!(socket_identity(&socket).unwrap(), identity);
        assert!(base::tokio::net::UnixStream::connect(&socket).await.is_ok());
        first_cancel.cancel();
        first.await.unwrap().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uds_regular_file_and_symlink_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = socket_test_root("non-socket");
        let regular = root.join("regular");
        std::fs::write(&regular, b"keep").unwrap();
        assert!(
            serve_uds(
                &regular,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                CancellationToken::new(),
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(&regular).unwrap(), b"keep");

        let target = root.join("target");
        std::fs::write(&target, b"target").unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(
            serve_uds(
                &link,
                Arc::new(UnsupportedDrainOwner::new("guard")),
                CancellationToken::new(),
            )
            .await
            .is_err()
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"target");
        std::fs::remove_dir_all(root).unwrap();
    }
}
