use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base::{
    sha2::{Digest, Sha256},
    tokio::sync::{Mutex, RwLock, mpsc},
    tokio_util::sync::CancellationToken,
    utils::rt::GlobalRuntime,
};
use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{Row, SqlitePool},
};
use gmv_nodec::{
    NodeEventSender,
    component_management::{
        AdmissionBarrier, ComponentDrainBehavior, ComponentProbeSnapshot, ComponentRuntimeHealth,
    },
};
use gmv_protocol::{
    avai::v1::{
        AiTaskResult, AiTaskState, CancelTaskRequest, CancelTaskResponse, CreateTaskRequest,
        CreateTaskResponse, ModelRef, QueryTaskRequest, QueryTaskResponse, VersionedPayload,
    },
    common::v1::{ErrorDetail, NodeIdentity, ResourceRef},
    guard::v1::{EventPriority, NodeEvent, NodeResourceSnapshot, ResourceReport, ResourceState},
};
use prost::Message;

use crate::feedback::{
    FeedbackConfig, FeedbackManager, FeedbackMaterial, safe_source_ref, sampled,
};
use crate::model::{
    ActiveModel, ModelError, ModelIdentity, ModelManager, RuntimeCallContext, RuntimeInput,
};
use crate::observability::{ActualModelIdentity, Observability, TaskTerminalOutcome};
use crate::source::{ResolvedImage, SourceError, SourcePolicy, SourceResolver};

const BUILTIN_CAPABILITY: &str = "image.metadata.inspect";
const MANAGED_INFERENCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct TaskManagerConfig {
    pub database_path: PathBuf,
    pub queue_size: usize,
    pub worker_count: usize,
    pub source_policy: SourcePolicy,
    pub max_result_bytes: usize,
}

impl Default for TaskManagerConfig {
    fn default() -> Self {
        Self {
            database_path: PathBuf::from("./data/avai.db"),
            queue_size: 128,
            worker_count: 2,
            source_policy: SourcePolicy::default(),
            max_result_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TaskError {}

impl TaskError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn internal(context: &str, error: impl std::fmt::Display) -> Self {
        base::log::error!(
            "Avai task storage failed: action=ai_task, stage={context}, error={error}"
        );
        Self::new("internal_failure", "Avai task storage operation failed")
    }
}

#[derive(Clone)]
pub struct TaskManager {
    identity: NodeIdentity,
    capabilities: Arc<HashSet<String>>,
    repository: TaskRepository,
    feedback: Option<Arc<FeedbackManager>>,
    queue: mpsc::Sender<String>,
    cancel: CancellationToken,
    workers: Arc<Mutex<Vec<base::tokio::task::JoinHandle<()>>>>,
    task_cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    event_sender: Arc<RwLock<Option<NodeEventSender>>>,
    running: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    admission: AdmissionBarrier,
    runtime_health: ComponentRuntimeHealth,
    #[cfg(test)]
    admission_pause: Arc<std::sync::Mutex<Option<AdmissionPause>>>,
}

#[cfg(test)]
#[derive(Clone)]
struct AdmissionPause {
    entered: Arc<base::tokio::sync::Semaphore>,
    release: Arc<base::tokio::sync::Semaphore>,
}

pub struct AvaiDrainBehavior(pub TaskManager);

struct InferenceOutput {
    result: AiTaskResult,
}

#[derive(Clone)]
struct TaskRepository {
    pool: SqlitePool,
    observability: Arc<Observability>,
}

struct InsertOutcome {
    created: bool,
    record: TaskRecord,
}

#[derive(Debug, Clone)]
struct TaskRecord {
    task_id: String,
    idempotency_key: String,
    request_hash: String,
    request: Vec<u8>,
    capability: String,
    route_id: String,
    state: AiTaskState,
    execution_binding: Option<Vec<u8>>,
    result: Option<AiTaskResult>,
    error_code: Option<String>,
    error_message: Option<String>,
}

fn validate_request(
    request: &CreateTaskRequest,
    identity: &NodeIdentity,
    capabilities: &HashSet<String>,
    now_epoch_ms: i64,
) -> Result<(), TaskError> {
    if request.task_id.trim().is_empty() {
        return Err(TaskError::new("invalid_task", "task_id is required"));
    }
    let operation = request
        .operation
        .as_ref()
        .filter(|operation| !operation.idempotency_key.trim().is_empty())
        .ok_or_else(|| TaskError::new("invalid_task", "operation.idempotency_key is required"))?;
    let _ = operation;
    let capability = request_capability(request);
    if capability.is_empty() || !capabilities.contains(capability) {
        return Err(TaskError::new(
            "capability_not_found",
            "requested capability is not advertised by this Avai node",
        ));
    }
    if request.source.is_none() {
        return Err(TaskError::new("invalid_source", "typed source is required"));
    }
    if request.deadline_epoch_ms != 0 && request.deadline_epoch_ms <= now_epoch_ms {
        return Err(TaskError::new("task_expired", "task deadline has expired"));
    }
    if let Some(expected) = &request.expected_avai
        && (expected.node_id != identity.node_id || expected.instance_id != identity.instance_id)
    {
        return Err(TaskError::new(
            "stale_instance",
            "Avai instance does not match the requested target",
        ));
    }
    Ok(())
}

fn request_hash(request: &CreateTaskRequest) -> String {
    let mut normalized = request.clone();
    normalized.operation = None;
    format!("{:x}", Sha256::digest(normalized.encode_to_vec()))
}

fn request_capability(request: &CreateTaskRequest) -> &str {
    if request.capability.is_empty() {
        &request.task_type
    } else {
        &request.capability
    }
}

fn source_task_error(error: SourceError) -> TaskError {
    TaskError::new(error.code, error.message)
}

fn error_detail(code: &str, message: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: message.to_string(),
        metadata: HashMap::new(),
    }
}

fn create_response(
    task_id: &str,
    state: AiTaskState,
    error: Option<ErrorDetail>,
) -> CreateTaskResponse {
    CreateTaskResponse {
        task_id: task_id.to_string(),
        state: state as i32,
        error,
    }
}

fn resource_state(state: AiTaskState) -> ResourceState {
    match state {
        AiTaskState::Pending => ResourceState::Starting,
        AiTaskState::Running => ResourceState::Running,
        AiTaskState::Succeeded | AiTaskState::Cancelled => ResourceState::Stopped,
        AiTaskState::Failed => ResourceState::Failed,
        AiTaskState::Unspecified => ResourceState::Unspecified,
    }
}

fn state_name(state: AiTaskState) -> &'static str {
    match state {
        AiTaskState::Unspecified => "unspecified",
        AiTaskState::Pending => "pending",
        AiTaskState::Running => "running",
        AiTaskState::Succeeded => "succeeded",
        AiTaskState::Failed => "failed",
        AiTaskState::Cancelled => "cancelled",
    }
}

fn terminal_outcome_name(outcome: TaskTerminalOutcome) -> &'static str {
    match outcome {
        TaskTerminalOutcome::Succeeded => "succeeded",
        TaskTerminalOutcome::Failed => "failed",
        TaskTerminalOutcome::Cancelled => "cancelled",
    }
}

fn model_ref_value(model: &ModelRef) -> String {
    format!(
        "{}@{}#{}:{}",
        model.model_id, model.version, model.revision, model.runtime
    )
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

mod binding;
mod execution;
mod lifecycle;
mod provider;
mod repository;

use binding::{ExecutionBinding, ExecutionKind};
use execution::{WorkerContext, worker_loop};
#[cfg(test)]
use provider::{BuiltinImageMetadataProvider, ImageInferenceProvider};
use provider::{ProviderManifest, ProviderRegistry};

#[cfg(test)]
#[path = "../../tests/unit/task/mod.rs"]
mod tests;
