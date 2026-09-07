use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
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
use gmv_nodec::NodeEventSender;
use gmv_protocol::{
    avai::v1::{
        AiTaskResult, AiTaskState, CancelTaskRequest, CancelTaskResponse, CreateTaskRequest,
        CreateTaskResponse, ModelRef, QueryTaskRequest, QueryTaskResponse, VersionedPayload,
    },
    common::v1::{ErrorDetail, NodeIdentity, ResourceRef},
    guard::v1::{EventPriority, NodeEvent, NodeResourceSnapshot, ResourceReport, ResourceState},
};
use prost::Message;

use crate::source::{ResolvedImage, SourceError, SourcePolicy, SourceResolver};

const BUILTIN_CAPABILITY: &str = "image.metadata.inspect";

#[derive(Debug, Clone)]
pub struct TaskManagerConfig {
    pub database_path: PathBuf,
    pub queue_size: usize,
    pub worker_count: usize,
    pub source_policy: SourcePolicy,
}

impl Default for TaskManagerConfig {
    fn default() -> Self {
        Self {
            database_path: PathBuf::from("./data/avai.db"),
            queue_size: 128,
            worker_count: 2,
            source_policy: SourcePolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    pub code: &'static str,
    pub message: String,
}

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
    queue: mpsc::Sender<String>,
    cancel: CancellationToken,
    workers: Arc<Mutex<Vec<base::tokio::task::JoinHandle<()>>>>,
    event_sender: Arc<RwLock<Option<NodeEventSender>>>,
    running: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

impl TaskManager {
    pub async fn open(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: TaskManagerConfig,
        runtime: &GlobalRuntime,
    ) -> Result<Self, TaskError> {
        if config.queue_size == 0 || config.worker_count == 0 {
            return Err(TaskError::new(
                "invalid_task_config",
                "queue_size and worker_count must be greater than zero",
            ));
        }
        let repository = TaskRepository::open(&config.database_path).await?;
        repository.recover_interrupted().await?;
        let resolver = SourceResolver::new(identity.clone(), config.source_policy, runtime)
            .map_err(source_task_error)?;
        let provider = Arc::new(ProviderRegistry::builtin(&capabilities)?);
        let capabilities = Arc::new(capabilities.into_iter().collect::<HashSet<_>>());
        let (queue, receiver) = mpsc::channel(config.queue_size);
        let receiver = Arc::new(Mutex::new(receiver));
        let cancel = runtime.cancel.child_token();
        let workers = Arc::new(Mutex::new(Vec::new()));
        let event_sender = Arc::new(RwLock::new(None));
        let running = Arc::new(AtomicUsize::new(0));

        for worker_id in 0..config.worker_count {
            let context = WorkerContext {
                identity: identity.clone(),
                repository: repository.clone(),
                resolver: resolver.clone(),
                provider: provider.clone(),
                receiver: receiver.clone(),
                cancel: cancel.clone(),
                event_sender: event_sender.clone(),
                running: running.clone(),
            };
            let handle = runtime
                .spawn(
                    format!("avai-task-worker-{worker_id}"),
                    worker_loop(context),
                )
                .map_err(|error| TaskError::internal("spawn_worker", error))?;
            workers.lock().await.push(handle);
        }

        let manager = Self {
            identity,
            capabilities,
            repository,
            queue,
            cancel,
            workers,
            event_sender,
            running,
            closed: Arc::new(AtomicBool::new(false)),
        };
        for task_id in manager.repository.pending_task_ids().await? {
            if manager.queue.send(task_id).await.is_err() {
                return Err(TaskError::new(
                    "executor_unavailable",
                    "task worker queue closed during recovery",
                ));
            }
        }
        Ok(manager)
    }

    pub async fn set_event_sender(&self, sender: NodeEventSender) {
        *self.event_sender.write().await = Some(sender);
    }

    pub fn running_task_count(&self) -> usize {
        self.running.load(Ordering::Acquire)
    }

    pub async fn create_task(
        &self,
        request: CreateTaskRequest,
        now_epoch_ms: i64,
    ) -> CreateTaskResponse {
        match self.create_task_inner(request, now_epoch_ms).await {
            Ok(record) => create_response(&record.task_id, record.state, record.error_detail()),
            Err(error) => create_response(
                "",
                AiTaskState::Failed,
                Some(error_detail(error.code, &error.message)),
            ),
        }
    }

    async fn create_task_inner(
        &self,
        request: CreateTaskRequest,
        now_epoch_ms: i64,
    ) -> Result<TaskRecord, TaskError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskError::new(
                "executor_unavailable",
                "Avai task manager is stopping",
            ));
        }
        validate_request(&request, &self.identity, &self.capabilities, now_epoch_ms)?;
        let request_hash = request_hash(&request);
        let task_id = request.task_id.clone();
        let inserted = self
            .repository
            .insert_or_get(&request, &request_hash, now_epoch_ms)
            .await?;
        if !inserted.created {
            return Ok(inserted.record);
        }
        match self.queue.try_send(task_id.clone()) {
            Ok(()) => Ok(inserted.record),
            Err(mpsc::error::TrySendError::Full(_)) => {
                let failed = self
                    .repository
                    .fail_pending(
                        &task_id,
                        "resource_exhausted",
                        "Avai task queue is full",
                        now_epoch_ms,
                    )
                    .await?;
                Ok(failed.unwrap_or(inserted.record))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let failed = self
                    .repository
                    .fail_pending(
                        &task_id,
                        "executor_unavailable",
                        "Avai task workers are unavailable",
                        now_epoch_ms,
                    )
                    .await?;
                Ok(failed.unwrap_or(inserted.record))
            }
        }
    }

    pub async fn cancel_task(&self, request: CancelTaskRequest) -> CancelTaskResponse {
        match self
            .repository
            .cancel(&request.task_id, now_epoch_ms())
            .await
        {
            Ok(Some(record)) => CancelTaskResponse {
                state: record.state as i32,
                error: record.error_detail(),
            },
            Ok(None) => CancelTaskResponse {
                state: AiTaskState::Cancelled as i32,
                error: None,
            },
            Err(error) => CancelTaskResponse {
                state: AiTaskState::Failed as i32,
                error: Some(error_detail(error.code, &error.message)),
            },
        }
    }

    pub async fn query_task(&self, request: QueryTaskRequest) -> QueryTaskResponse {
        match self.repository.get(&request.task_id).await {
            Ok(Some(record)) => record.query_response(),
            Ok(None) => QueryTaskResponse {
                task_id: request.task_id,
                state: AiTaskState::Failed as i32,
                result: Vec::new(),
                error: Some(error_detail("task_not_found", "task does not exist")),
                typed_result: None,
            },
            Err(error) => QueryTaskResponse {
                task_id: request.task_id,
                state: AiTaskState::Failed as i32,
                result: Vec::new(),
                error: Some(error_detail(error.code, &error.message)),
                typed_result: None,
            },
        }
    }

    pub async fn resource_snapshot(&self) -> NodeResourceSnapshot {
        match self.repository.list().await {
            Ok(tasks) => NodeResourceSnapshot {
                full: true,
                resources: tasks
                    .into_iter()
                    .map(|task| ResourceReport {
                        resource: Some(ResourceRef {
                            resource_id: task.task_id,
                            resource_type: "ai_task".to_string(),
                        }),
                        state: resource_state(task.state) as i32,
                        labels: HashMap::from([
                            ("capability".to_string(), task.capability),
                            ("route_id".to_string(), task.route_id),
                        ]),
                    })
                    .collect(),
            },
            Err(error) => {
                base::log::error!(
                    "Avai snapshot failed: action=ai_task, stage=snapshot, error_code={}",
                    error.code
                );
                NodeResourceSnapshot {
                    full: false,
                    resources: Vec::new(),
                }
            }
        }
    }

    pub async fn close_and_wait(&self) -> Result<(), TaskError> {
        let already_closed = self.closed.swap(true, Ordering::AcqRel);
        self.cancel.cancel();
        if already_closed {
            return Ok(());
        }
        let workers = std::mem::take(&mut *self.workers.lock().await);
        for worker in workers {
            worker
                .await
                .map_err(|error| TaskError::internal("join_worker", error))?;
        }
        self.repository.close().await;
        Ok(())
    }
}

struct WorkerContext {
    identity: NodeIdentity,
    repository: TaskRepository,
    resolver: SourceResolver,
    provider: Arc<ProviderRegistry>,
    receiver: Arc<Mutex<mpsc::Receiver<String>>>,
    cancel: CancellationToken,
    event_sender: Arc<RwLock<Option<NodeEventSender>>>,
    running: Arc<AtomicUsize>,
}

async fn worker_loop(context: WorkerContext) {
    loop {
        let task_id = {
            let mut receiver = context.receiver.lock().await;
            base::tokio::select! {
                _ = context.cancel.cancelled() => return,
                task_id = receiver.recv() => task_id,
            }
        };
        let Some(task_id) = task_id else {
            return;
        };
        context.running.fetch_add(1, Ordering::AcqRel);
        let result = base::tokio::select! {
            _ = context.cancel.cancelled() => None,
            result = process_task(&context, &task_id) => Some(result),
        };
        context.running.fetch_sub(1, Ordering::AcqRel);
        match result {
            Some(Ok(Some(record))) => emit_terminal_event(&context, &record).await,
            Some(Ok(None)) => {}
            Some(Err(error)) => {
                base::log::error!(
                    "Avai task execution failed: action=ai_task, stage=worker, task_id={}, error_code={}, error={}",
                    task_id,
                    error.code,
                    error.message
                );
            }
            None => return,
        }
    }
}

async fn process_task(
    context: &WorkerContext,
    task_id: &str,
) -> Result<Option<TaskRecord>, TaskError> {
    let Some(record) = context.repository.claim(task_id, now_epoch_ms()).await? else {
        return Ok(None);
    };
    let request = CreateTaskRequest::decode(record.request.as_slice())
        .map_err(|error| TaskError::internal("decode_request", error))?;
    let source = request
        .source
        .as_ref()
        .ok_or_else(|| TaskError::new("invalid_source", "persisted task has no typed source"));
    let terminal = match source {
        Ok(source) => match context
            .resolver
            .resolve(source, &record.capability, now_epoch_ms())
            .await
        {
            Ok(image) => match context
                .provider
                .infer(&record.capability, image, request.requested_model.as_ref())
                .await
            {
                Ok(output) => {
                    context
                        .repository
                        .succeed(task_id, output, now_epoch_ms())
                        .await?
                }
                Err(error) => {
                    context
                        .repository
                        .fail_running(task_id, error.code, &error.message, now_epoch_ms())
                        .await?
                }
            },
            Err(error) => {
                context
                    .repository
                    .fail_running(task_id, error.code, &error.message, now_epoch_ms())
                    .await?
            }
        },
        Err(error) => {
            context
                .repository
                .fail_running(task_id, error.code, &error.message, now_epoch_ms())
                .await?
        }
    };
    Ok(terminal)
}

async fn emit_terminal_event(context: &WorkerContext, record: &TaskRecord) {
    let Some(sender) = context.event_sender.read().await.clone() else {
        return;
    };
    let payload = base::serde_json::to_vec(&base::serde_json::json!({
        "task_id": record.task_id,
        "capability": record.capability,
        "route_id": record.route_id,
        "state": state_name(record.state),
        "result_available": record.result.is_some(),
        "error_code": record.error_code,
        "avai_node_id": context.identity.node_id,
        "avai_instance_id": context.identity.instance_id,
    }))
    .unwrap_or_default();
    let event = NodeEvent {
        event_id: format!("avai-task-{}-terminal", record.task_id),
        topic: "avai.task.terminal".to_string(),
        priority: EventPriority::P1 as i32,
        payload,
    };
    if let Err(error) = sender.try_send(event) {
        base::log::warn!(
            "Avai terminal event enqueue failed: action=ai_task, stage=event, task_id={}, reason={error}",
            record.task_id
        );
    }
}

trait ImageInferenceProvider: Send + Sync {
    fn manifest(&self) -> ProviderManifest;

    fn infer<'a>(
        &'a self,
        capability: &'a str,
        image: ResolvedImage,
        requested_model: Option<&'a ModelRef>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<InferenceOutput, TaskError>> + Send + 'a>,
    >;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderManifest {
    capability: &'static str,
    model_id: &'static str,
    model_version: &'static str,
    runtime: &'static str,
    result_schema: &'static str,
    result_schema_version: u32,
}

struct ProviderRegistry {
    providers: Vec<Arc<dyn ImageInferenceProvider>>,
}

impl ProviderRegistry {
    fn builtin(configured_capabilities: &[String]) -> Result<Self, TaskError> {
        let providers: Vec<Arc<dyn ImageInferenceProvider>> =
            vec![Arc::new(BuiltinImageMetadataProvider)];
        let installed = providers
            .iter()
            .map(|provider| provider.manifest().capability)
            .collect::<HashSet<_>>();
        if let Some(capability) = configured_capabilities
            .iter()
            .find(|capability| !installed.contains(capability.as_str()))
        {
            return Err(TaskError::new(
                "invalid_task_config",
                format!("configured capability has no installed provider: {capability}"),
            ));
        }
        Ok(Self { providers })
    }
}

impl ProviderRegistry {
    async fn infer(
        &self,
        capability: &str,
        image: ResolvedImage,
        requested_model: Option<&ModelRef>,
    ) -> Result<InferenceOutput, TaskError> {
        let provider = self
            .providers
            .iter()
            .find(|provider| {
                let manifest = provider.manifest();
                manifest.capability == capability
                    && requested_model.is_none_or(|requested| {
                        requested.model_id == manifest.model_id
                            && requested.version == manifest.model_version
                            && (requested.runtime.is_empty()
                                || requested.runtime == manifest.runtime)
                    })
            })
            .ok_or_else(|| {
                TaskError::new(
                    "model_not_found",
                    "no installed inference provider matches the requested capability and model",
                )
            })?;
        provider.infer(capability, image, requested_model).await
    }
}

struct BuiltinImageMetadataProvider;

impl ImageInferenceProvider for BuiltinImageMetadataProvider {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            capability: BUILTIN_CAPABILITY,
            model_id: "builtin.image-metadata",
            model_version: "1",
            runtime: "rust-image",
            result_schema: "gmv.image.metadata.inspect",
            result_schema_version: 1,
        }
    }

    fn infer<'a>(
        &'a self,
        capability: &'a str,
        image: ResolvedImage,
        requested_model: Option<&'a ModelRef>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<InferenceOutput, TaskError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let manifest = self.manifest();
            if capability != manifest.capability {
                return Err(TaskError::new(
                    "model_not_found",
                    "no installed inference provider implements the requested capability",
                ));
            }
            if requested_model.is_some_and(|model| {
                model.model_id != manifest.model_id
                    || model.version != manifest.model_version
                    || (!model.runtime.is_empty() && model.runtime != manifest.runtime)
            }) {
                return Err(TaskError::new(
                    "model_not_found",
                    "requested model is not installed",
                ));
            }
            let payload = base::serde_json::to_vec(&base::serde_json::json!({
                "content_type": image.content_type,
                "size_bytes": image.bytes.len(),
                "sha256": image.sha256,
                "width": image.width,
                "height": image.height,
                "source_identity": image.source_identity,
            }))
            .map_err(|error| TaskError::internal("encode_result", error))?;
            Ok(InferenceOutput {
                result: AiTaskResult {
                    output: Some(VersionedPayload {
                        schema: manifest.result_schema.to_string(),
                        version: manifest.result_schema_version,
                        json: payload,
                    }),
                    actual_model: Some(ModelRef {
                        model_id: manifest.model_id.to_string(),
                        version: manifest.model_version.to_string(),
                        runtime: manifest.runtime.to_string(),
                    }),
                    evidence: Vec::new(),
                    completed_at_epoch_ms: now_epoch_ms(),
                },
            })
        })
    }
}

struct InferenceOutput {
    result: AiTaskResult,
}

#[derive(Clone)]
struct TaskRepository {
    pool: SqlitePool,
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
    result: Option<AiTaskResult>,
    error_code: Option<String>,
    error_message: Option<String>,
}

impl TaskRecord {
    fn error_detail(&self) -> Option<ErrorDetail> {
        self.error_code.as_deref().map(|code| {
            error_detail(
                code,
                self.error_message.as_deref().unwrap_or("Avai task failed"),
            )
        })
    }

    fn query_response(self) -> QueryTaskResponse {
        let error = self.error_detail();
        let result = self
            .result
            .as_ref()
            .and_then(|result| result.output.as_ref())
            .map_or_else(Vec::new, |output| output.json.clone());
        QueryTaskResponse {
            task_id: self.task_id,
            state: self.state as i32,
            result,
            error,
            typed_result: self.result,
        }
    }
}

impl TaskRepository {
    async fn open(path: &Path) -> Result<Self, TaskError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| TaskError::internal("create_database_directory", error))?;
        }
        let pool_config = DatabasePoolConfig {
            max_size: 4,
            min_idle: Some(1),
            ..Default::default()
        };
        let pool = base_db::dbx::sqlitex::build_sqlite_pool(
            SqliteConnectionConfig::new(path),
            pool_config,
        )
        .map_err(|error| TaskError::internal("configure_database", error))?;
        base_db::sqlx::query(
            "CREATE TABLE IF NOT EXISTS avai_task (\
             task_id TEXT PRIMARY KEY NOT NULL,\
             idempotency_key TEXT NOT NULL UNIQUE,\
             request_hash TEXT NOT NULL,\
             request BLOB NOT NULL,\
             capability TEXT NOT NULL,\
             route_id TEXT NOT NULL,\
             state INTEGER NOT NULL,\
             result BLOB NULL,\
             error_code TEXT NULL,\
             error_message TEXT NULL,\
             created_at_ms INTEGER NOT NULL,\
             updated_at_ms INTEGER NOT NULL,\
             terminal_at_ms INTEGER NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|error| TaskError::internal("initialize_schema", error))?;
        Ok(Self { pool })
    }

    async fn recover_interrupted(&self) -> Result<(), TaskError> {
        base_db::sqlx::query("UPDATE avai_task SET state=?, updated_at_ms=? WHERE state=?")
            .bind(AiTaskState::Pending as i32)
            .bind(now_epoch_ms())
            .bind(AiTaskState::Running as i32)
            .execute(&self.pool)
            .await
            .map_err(|error| TaskError::internal("recover_interrupted", error))?;
        Ok(())
    }

    async fn pending_task_ids(&self) -> Result<Vec<String>, TaskError> {
        let rows = base_db::sqlx::query(
            "SELECT task_id FROM avai_task WHERE state=? ORDER BY created_at_ms, task_id",
        )
        .bind(AiTaskState::Pending as i32)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| TaskError::internal("load_pending", error))?;
        rows.into_iter()
            .map(|row| {
                row.try_get("task_id")
                    .map_err(|error| TaskError::internal("decode_pending", error))
            })
            .collect()
    }

    async fn insert_or_get(
        &self,
        request: &CreateTaskRequest,
        request_hash: &str,
        now_epoch_ms: i64,
    ) -> Result<InsertOutcome, TaskError> {
        let operation = request.operation.as_ref().expect("validated operation");
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| TaskError::internal("begin_create", error))?;
        if let Some(existing) = self
            .get_in(&mut transaction, "task_id", &request.task_id)
            .await?
        {
            return existing_outcome(transaction, existing, operation, request_hash).await;
        }
        if let Some(existing) = self
            .get_in(
                &mut transaction,
                "idempotency_key",
                &operation.idempotency_key,
            )
            .await?
        {
            return existing_outcome(transaction, existing, operation, request_hash).await;
        }
        let encoded = request.encode_to_vec();
        base_db::sqlx::query(
            "INSERT INTO avai_task(task_id,idempotency_key,request_hash,request,capability,route_id,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(&request.task_id)
        .bind(&operation.idempotency_key)
        .bind(request_hash)
        .bind(encoded)
        .bind(request_capability(request))
        .bind(&request.route_id)
        .bind(AiTaskState::Pending as i32)
        .bind(now_epoch_ms)
        .bind(now_epoch_ms)
        .execute(&mut *transaction)
        .await
        .map_err(|error| TaskError::internal("insert_task", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| TaskError::internal("commit_create", error))?;
        Ok(InsertOutcome {
            created: true,
            record: TaskRecord {
                task_id: request.task_id.clone(),
                idempotency_key: operation.idempotency_key.clone(),
                request_hash: request_hash.to_string(),
                request: request.encode_to_vec(),
                capability: request_capability(request).to_string(),
                route_id: request.route_id.clone(),
                state: AiTaskState::Pending,
                result: None,
                error_code: None,
                error_message: None,
            },
        })
    }

    async fn get_in(
        &self,
        transaction: &mut base_db::sqlx::Transaction<'_, base_db::sqlx::Sqlite>,
        column: &str,
        value: &str,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let statement = match column {
            "task_id" => SELECT_TASK_BY_ID,
            "idempotency_key" => SELECT_TASK_BY_IDEMPOTENCY,
            _ => unreachable!("fixed repository column"),
        };
        let row = base_db::sqlx::query(statement)
            .bind(value)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| TaskError::internal("query_existing", error))?;
        row.map(decode_task_row).transpose()
    }

    async fn get(&self, task_id: &str) -> Result<Option<TaskRecord>, TaskError> {
        base_db::sqlx::query(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| TaskError::internal("query_task", error))?
            .map(decode_task_row)
            .transpose()
    }

    async fn list(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let rows = base_db::sqlx::query(SELECT_ALL_TASKS)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| TaskError::internal("list_tasks", error))?;
        rows.into_iter().map(decode_task_row).collect()
    }

    async fn claim(&self, task_id: &str, now_ms: i64) -> Result<Option<TaskRecord>, TaskError> {
        let updated = base_db::sqlx::query(
            "UPDATE avai_task SET state=?, updated_at_ms=? WHERE task_id=? AND state=?",
        )
        .bind(AiTaskState::Running as i32)
        .bind(now_ms)
        .bind(task_id)
        .bind(AiTaskState::Pending as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("claim_task", error))?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.get(task_id).await
    }

    async fn succeed(
        &self,
        task_id: &str,
        output: InferenceOutput,
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let encoded = output.result.encode_to_vec();
        let updated = base_db::sqlx::query(
            "UPDATE avai_task SET state=?, result=?, error_code=NULL, error_message=NULL, updated_at_ms=?, terminal_at_ms=? WHERE task_id=? AND state=?",
        )
        .bind(AiTaskState::Succeeded as i32)
        .bind(encoded)
        .bind(now_ms)
        .bind(now_ms)
        .bind(task_id)
        .bind(AiTaskState::Running as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("complete_task", error))?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.get(task_id).await
    }

    async fn fail_pending(
        &self,
        task_id: &str,
        code: &str,
        message: &str,
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        self.fail_from_state(task_id, code, message, now_ms, AiTaskState::Pending)
            .await
    }

    async fn fail_running(
        &self,
        task_id: &str,
        code: &str,
        message: &str,
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        self.fail_from_state(task_id, code, message, now_ms, AiTaskState::Running)
            .await
    }

    async fn fail_from_state(
        &self,
        task_id: &str,
        code: &str,
        message: &str,
        now_ms: i64,
        expected: AiTaskState,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let updated = base_db::sqlx::query(
            "UPDATE avai_task SET state=?, error_code=?, error_message=?, updated_at_ms=?, terminal_at_ms=? WHERE task_id=? AND state=?",
        )
        .bind(AiTaskState::Failed as i32)
        .bind(code)
        .bind(message)
        .bind(now_ms)
        .bind(now_ms)
        .bind(task_id)
        .bind(expected as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("fail_task", error))?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.get(task_id).await
    }

    async fn cancel(&self, task_id: &str, now_ms: i64) -> Result<Option<TaskRecord>, TaskError> {
        base_db::sqlx::query(
            "UPDATE avai_task SET state=?, updated_at_ms=?, terminal_at_ms=? WHERE task_id=? AND state IN (?,?)",
        )
        .bind(AiTaskState::Cancelled as i32)
        .bind(now_ms)
        .bind(now_ms)
        .bind(task_id)
        .bind(AiTaskState::Pending as i32)
        .bind(AiTaskState::Running as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("cancel_task", error))?;
        self.get(task_id).await
    }

    async fn close(&self) {
        self.pool.close().await;
    }
}

async fn existing_outcome(
    transaction: base_db::sqlx::Transaction<'_, base_db::sqlx::Sqlite>,
    existing: TaskRecord,
    operation: &gmv_protocol::common::v1::OperationRef,
    request_hash: &str,
) -> Result<InsertOutcome, TaskError> {
    transaction
        .rollback()
        .await
        .map_err(|error| TaskError::internal("rollback_duplicate", error))?;
    if existing.idempotency_key == operation.idempotency_key
        && existing.request_hash == request_hash
    {
        Ok(InsertOutcome {
            created: false,
            record: existing,
        })
    } else {
        Err(TaskError::new(
            "task_conflict",
            "task identity or idempotency key is already bound to another request",
        ))
    }
}

const SELECT_TASK_BY_ID: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,result,error_code,error_message FROM avai_task WHERE task_id=?";
const SELECT_TASK_BY_IDEMPOTENCY: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,result,error_code,error_message FROM avai_task WHERE idempotency_key=?";
const SELECT_ALL_TASKS: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,result,error_code,error_message FROM avai_task ORDER BY created_at_ms,task_id";

fn decode_task_row(row: base_db::sqlx::sqlite::SqliteRow) -> Result<TaskRecord, TaskError> {
    let state: i32 = row
        .try_get("state")
        .map_err(|error| TaskError::internal("decode_task_state", error))?;
    let result: Option<Vec<u8>> = row
        .try_get("result")
        .map_err(|error| TaskError::internal("decode_task_result", error))?;
    let result = result
        .map(|encoded| {
            AiTaskResult::decode(encoded.as_slice())
                .map_err(|error| TaskError::internal("decode_typed_result", error))
        })
        .transpose()?;
    Ok(TaskRecord {
        task_id: row
            .try_get("task_id")
            .map_err(|error| TaskError::internal("decode_task_id", error))?,
        idempotency_key: row
            .try_get("idempotency_key")
            .map_err(|error| TaskError::internal("decode_idempotency_key", error))?,
        request_hash: row
            .try_get("request_hash")
            .map_err(|error| TaskError::internal("decode_request_hash", error))?,
        request: row
            .try_get("request")
            .map_err(|error| TaskError::internal("decode_request", error))?,
        capability: row
            .try_get("capability")
            .map_err(|error| TaskError::internal("decode_capability", error))?,
        route_id: row
            .try_get("route_id")
            .map_err(|error| TaskError::internal("decode_route_id", error))?,
        state: AiTaskState::try_from(state).unwrap_or(AiTaskState::Failed),
        result,
        error_code: row
            .try_get("error_code")
            .map_err(|error| TaskError::internal("decode_error_code", error))?,
        error_message: row
            .try_get("error_message")
            .map_err(|error| TaskError::internal("decode_error_message", error))?,
    })
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

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use base::net::{
        transport::MessageTransport,
        uds::{ManagedUnixStreamListener, UnixTransportConfig},
    };
    #[cfg(unix)]
    use gmv_protocol::session::v1::{ReadGrantedImageRequest, ReadGrantedImageResponse};
    use gmv_protocol::{
        avai::v1::{ImageMetadata, ImageUrlSource, OwnedImageRef, SourceSpec, source_spec},
        common::v1::{
            AccessGrant, DataEndpoint, NodeKind, OperationRef, ResourceRef, TransportCapabilities,
            TransportMode,
        },
    };
    use std::io::Write;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

    fn test_identity() -> NodeIdentity {
        NodeIdentity {
            node_id: "avai-test".to_string(),
            instance_id: "instance-test".to_string(),
            kind: NodeKind::Avai as i32,
        }
    }

    fn test_request_with_source(task_id: &str, source: SourceSpec) -> CreateTaskRequest {
        CreateTaskRequest {
            operation: Some(OperationRef {
                operation_id: format!("operation-{task_id}"),
                idempotency_key: format!("idempotency-{task_id}"),
            }),
            task_id: task_id.to_string(),
            capability: BUILTIN_CAPABILITY.to_string(),
            expected_avai: Some(test_identity()),
            source: Some(source),
            ..Default::default()
        }
    }

    fn test_request(task_id: &str) -> CreateTaskRequest {
        test_request_with_source(
            task_id,
            SourceSpec {
                source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
                    url: "http://127.0.0.1/image.png".to_string(),
                    expected: None,
                    max_bytes: 1024,
                })),
            },
        )
    }

    async fn test_manager() -> (TaskManager, PathBuf) {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("avai-task-test-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(
            format!("avai-task-test-{id}"),
        ))
        .unwrap();
        let manager = TaskManager::open(
            test_identity(),
            vec![BUILTIN_CAPABILITY.to_string()],
            TaskManagerConfig {
                database_path: root.join("avai.db"),
                worker_count: 1,
                queue_size: 8,
                source_policy: SourcePolicy {
                    allow_private_image_urls: false,
                    ..SourcePolicy::default()
                },
            },
            &runtime,
        )
        .await
        .unwrap();
        (manager, root)
    }

    fn png_bytes() -> Vec<u8> {
        use base::base64::Engine;
        base::base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
    }

    fn metadata(bytes: &[u8]) -> ImageMetadata {
        ImageMetadata {
            content_type: "image/png".to_string(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            width: 1,
            height: 1,
        }
    }

    async fn wait_terminal(manager: &TaskManager, task_id: &str) -> QueryTaskResponse {
        for _ in 0..100 {
            let query = manager
                .query_task(QueryTaskRequest {
                    task_id: task_id.to_string(),
                })
                .await;
            if matches!(
                AiTaskState::try_from(query.state),
                Ok(AiTaskState::Succeeded | AiTaskState::Failed | AiTaskState::Cancelled)
            ) {
                return query;
            }
            base::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("task did not reach a terminal state: {task_id}");
    }

    #[test]
    fn configured_capability_requires_an_installed_provider_manifest() {
        let error = ProviderRegistry::builtin(&["asset.damage.detect".to_string()])
            .err()
            .unwrap();
        assert_eq!(error.code, "invalid_task_config");
    }

    async fn serve_image_once(bytes: Vec<u8>) -> (String, base::tokio::task::JoinHandle<()>) {
        use base::tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let handle = base::tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
        });
        (format!("http://{address}/image.png"), handle)
    }

    #[cfg(unix)]
    async fn serve_uds_image_once(
        root: &Path,
        bytes: Vec<u8>,
        runtime: &GlobalRuntime,
    ) -> (String, base::tokio::task::JoinHandle<()>) {
        let socket_path = root.join("session-image.sock");
        let mut config = UnixTransportConfig::new(root, &socket_path);
        config.max_message_size = 1024;
        let listener = ManagedUnixStreamListener::bind(config).await.unwrap();
        let task_runtime = runtime.clone();
        let handle = base::tokio::spawn(async move {
            let (connection, _) = listener
                .accept(&task_runtime, "avai-source-test-uds-io")
                .await
                .unwrap();
            let request = connection.receive().await.unwrap();
            let request = ReadGrantedImageRequest::decode(request.payload).unwrap();
            assert_eq!(request.grant_id, "grant-1");
            assert_eq!(request.proof, vec![1, 2, 3]);
            assert_eq!(request.image_id, "snapshot-uds-1");
            let response = ReadGrantedImageResponse {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                image: bytes,
                content_type: "image/png".to_string(),
                error: None,
            };
            connection
                .send(base::bytes::Bytes::from(response.encode_to_vec()))
                .await
                .unwrap();
            let _ =
                base::tokio::time::timeout(std::time::Duration::from_secs(1), connection.receive())
                    .await;
            connection.close_and_wait().await.unwrap();
            listener.close_and_wait().await.unwrap();
        });
        (format!("unix://{}", socket_path.display()), handle)
    }

    #[tokio::test]
    async fn create_is_idempotent_and_conflicting_request_is_rejected() {
        let (manager, root) = test_manager().await;
        let request = test_request("task-1");
        let first = manager.create_task(request.clone(), now_epoch_ms()).await;
        assert_eq!(first.task_id, "task-1");
        assert_eq!(first.state, AiTaskState::Pending as i32);
        let repeated = manager.create_task(request.clone(), now_epoch_ms()).await;
        assert_eq!(repeated.task_id, "task-1");

        let mut conflict = request;
        conflict.route_id = "different".to_string();
        let conflict = manager.create_task(conflict, now_epoch_ms()).await;
        assert_eq!(conflict.state, AiTaskState::Failed as i32);
        assert_eq!(conflict.error.unwrap().code, "task_conflict");
        manager.close_and_wait().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn interrupted_running_task_is_requeued_after_repository_reopen() {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("avai-recovery-test-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let database_path = root.join("avai.db");
        let repository = TaskRepository::open(&database_path).await.unwrap();
        let request = test_request("task-recovery");
        repository
            .insert_or_get(&request, &request_hash(&request), 1)
            .await
            .unwrap();
        assert_eq!(
            repository
                .claim("task-recovery", 2)
                .await
                .unwrap()
                .unwrap()
                .state,
            AiTaskState::Running
        );
        repository.pool.close().await;

        let reopened = TaskRepository::open(&database_path).await.unwrap();
        reopened.recover_interrupted().await.unwrap();
        assert_eq!(
            reopened.get("task-recovery").await.unwrap().unwrap().state,
            AiTaskState::Pending
        );
        assert_eq!(
            reopened.pending_task_ids().await.unwrap(),
            vec!["task-recovery".to_string()]
        );
        reopened.pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stream_source_has_a_stable_rejection() {
        let (manager, root) = test_manager().await;
        let mut request = test_request("task-stream");
        request.source = Some(SourceSpec {
            source: Some(source_spec::Source::StreamFrame(Default::default())),
        });
        let response = manager.create_task(request, now_epoch_ms()).await;
        assert_eq!(response.state, AiTaskState::Pending as i32);
        for _ in 0..50 {
            let query = manager
                .query_task(QueryTaskRequest {
                    task_id: "task-stream".to_string(),
                })
                .await;
            if query.state == AiTaskState::Failed as i32 {
                assert_eq!(query.error.unwrap().code, "source_transport_unsupported");
                manager.close_and_wait().await.unwrap();
                std::fs::remove_dir_all(root).unwrap();
                return;
            }
            base::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("stream source task did not reach a terminal state");
    }

    #[tokio::test]
    async fn local_object_url_and_session_owned_sources_share_the_same_pipeline() {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("avai-source-test-{}-{id}", std::process::id()));
        let object_root = root.join("objects");
        std::fs::create_dir_all(&object_root).unwrap();
        let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(
            format!("avai-source-test-{id}"),
        ))
        .unwrap();
        let manager = TaskManager::open(
            test_identity(),
            vec![BUILTIN_CAPABILITY.to_string()],
            TaskManagerConfig {
                database_path: root.join("avai.db"),
                worker_count: 2,
                queue_size: 8,
                source_policy: SourcePolicy {
                    allow_private_image_urls: true,
                    allowed_internal_hosts: HashSet::from(["127.0.0.1".to_string()]),
                    object_root: object_root.clone(),
                    uds_socket_root: root.join("run"),
                    ..SourcePolicy::default()
                },
            },
            &runtime,
        )
        .await
        .unwrap();
        let bytes = png_bytes();
        let mut object = std::fs::File::create(object_root.join("object-1")).unwrap();
        object.write_all(&bytes).unwrap();
        drop(object);
        let grant = |endpoint: String| AccessGrant {
            grant_id: "grant-1".to_string(),
            expected_consumer: Some(test_identity()),
            purpose: BUILTIN_CAPABILITY.to_string(),
            expires_at_epoch_ms: now_epoch_ms() + 60_000,
            endpoints: vec![DataEndpoint {
                name: "image".to_string(),
                uri: endpoint,
                capabilities: Some(TransportCapabilities {
                    reliable: true,
                    ordered: true,
                    preserves_message_boundary: false,
                    encrypted: false,
                    congestion_controlled: true,
                    local_only: true,
                    max_message_size: 1024,
                    mode: TransportMode::Stream as i32,
                }),
                labels: HashMap::new(),
            }],
            proof: vec![1, 2, 3],
        };
        let local_source = SourceSpec {
            source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                owner: Some(test_identity()),
                resource: Some(ResourceRef {
                    resource_id: "object-1".to_string(),
                    resource_type: "avai_image".to_string(),
                }),
                metadata: Some(metadata(&bytes)),
                access: Some(grant("gmv-object://object-1".to_string())),
            })),
        };
        let response = manager
            .create_task(
                test_request_with_source("task-object", local_source),
                now_epoch_ms(),
            )
            .await;
        assert_eq!(response.state, AiTaskState::Pending as i32);
        assert_eq!(
            wait_terminal(&manager, "task-object").await.state,
            AiTaskState::Succeeded as i32
        );

        let (url, url_server) = serve_image_once(bytes.clone()).await;
        let url_source = SourceSpec {
            source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
                url,
                expected: Some(metadata(&bytes)),
                max_bytes: 1024,
            })),
        };
        manager
            .create_task(
                test_request_with_source("task-url", url_source),
                now_epoch_ms(),
            )
            .await;
        assert_eq!(
            wait_terminal(&manager, "task-url").await.state,
            AiTaskState::Succeeded as i32
        );
        url_server.await.unwrap();

        let (session_url, session_server) = serve_image_once(bytes.clone()).await;
        let session_source = SourceSpec {
            source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                owner: Some(NodeIdentity {
                    node_id: "session-1".to_string(),
                    instance_id: "session-instance-1".to_string(),
                    kind: NodeKind::Session as i32,
                }),
                resource: Some(ResourceRef {
                    resource_id: "snapshot-1".to_string(),
                    resource_type: "gb28181_image".to_string(),
                }),
                metadata: Some(metadata(&bytes)),
                access: Some(grant(session_url)),
            })),
        };
        manager
            .create_task(
                test_request_with_source("task-session", session_source),
                now_epoch_ms(),
            )
            .await;
        let session_result = wait_terminal(&manager, "task-session").await;
        assert_eq!(session_result.state, AiTaskState::Succeeded as i32);
        assert_eq!(
            session_result
                .typed_result
                .unwrap()
                .actual_model
                .unwrap()
                .model_id,
            "builtin.image-metadata"
        );
        session_server.await.unwrap();

        #[cfg(unix)]
        {
            let (session_uds, session_uds_server) =
                serve_uds_image_once(&root.join("run"), bytes.clone(), &runtime).await;
            let session_uds_source = SourceSpec {
                source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                    owner: Some(NodeIdentity {
                        node_id: "session-1".to_string(),
                        instance_id: "session-instance-1".to_string(),
                        kind: NodeKind::Session as i32,
                    }),
                    resource: Some(ResourceRef {
                        resource_id: "snapshot-uds-1".to_string(),
                        resource_type: "gb28181_image".to_string(),
                    }),
                    metadata: Some(metadata(&bytes)),
                    access: Some(grant(session_uds)),
                })),
            };
            manager
                .create_task(
                    test_request_with_source("task-session-uds", session_uds_source),
                    now_epoch_ms(),
                )
                .await;
            assert_eq!(
                wait_terminal(&manager, "task-session-uds").await.state,
                AiTaskState::Succeeded as i32
            );
            session_uds_server.await.unwrap();
        }

        manager.close_and_wait().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
