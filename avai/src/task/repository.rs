use super::*;

impl TaskRecord {
    pub(super) fn error_detail(&self) -> Option<ErrorDetail> {
        self.error_code.as_deref().map(|code| {
            error_detail(
                code,
                self.error_message.as_deref().unwrap_or("Avai task failed"),
            )
        })
    }

    pub(super) fn query_response(self) -> QueryTaskResponse {
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
    #[cfg(test)]
    pub(super) async fn insert_nonterminal_task_for_test(
        &self,
        task_id: &str,
    ) -> Result<(), TaskError> {
        base_db::sqlx::query(
            "INSERT INTO avai_task(task_id,idempotency_key,request_hash,request,capability,\
             route_id,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(task_id)
        .bind(format!("idempotency-{task_id}"))
        .bind("test-request-hash")
        .bind(Vec::<u8>::new())
        .bind("vehicle.detect")
        .bind("test-route")
        .bind(AiTaskState::Pending as i32)
        .bind(now_epoch_ms())
        .bind(now_epoch_ms())
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("insert_test_nonterminal", error))?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn open(path: &Path) -> Result<Self, TaskError> {
        Self::open_with_observability(path, Arc::new(Observability::new())).await
    }

    pub(super) async fn open_with_observability(
        path: &Path,
        observability: Arc<Observability>,
    ) -> Result<Self, TaskError> {
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
             execution_binding BLOB NULL,\
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
        let columns = base_db::sqlx::query("PRAGMA table_info(avai_task)")
            .fetch_all(&pool)
            .await
            .map_err(|error| TaskError::internal("inspect_schema", error))?;
        let has_execution_binding = columns.iter().any(|row| {
            row.try_get::<String, _>("name")
                .is_ok_and(|name| name == "execution_binding")
        });
        if !has_execution_binding {
            base_db::sqlx::query("ALTER TABLE avai_task ADD COLUMN execution_binding BLOB NULL")
                .execute(&pool)
                .await
                .map_err(|error| TaskError::internal("upgrade_execution_binding", error))?;
        }
        Ok(Self {
            pool,
            observability,
        })
    }

    pub(super) async fn recover_interrupted(&self) -> Result<(), TaskError> {
        base_db::sqlx::query("UPDATE avai_task SET state=?, updated_at_ms=? WHERE state=?")
            .bind(AiTaskState::Pending as i32)
            .bind(now_epoch_ms())
            .bind(AiTaskState::Running as i32)
            .execute(&self.pool)
            .await
            .map_err(|error| TaskError::internal("recover_interrupted", error))?;
        Ok(())
    }

    pub(super) async fn pending_task_ids(&self) -> Result<Vec<String>, TaskError> {
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

    pub(super) async fn nonterminal_task_count(&self) -> Result<usize, TaskError> {
        let count: i64 =
            base_db::sqlx::query_scalar("SELECT COUNT(*) FROM avai_task WHERE state IN (?, ?)")
                .bind(AiTaskState::Pending as i32)
                .bind(AiTaskState::Running as i32)
                .fetch_one(&self.pool)
                .await
                .map_err(|error| TaskError::internal("count_nonterminal", error))?;
        usize::try_from(count).map_err(|error| TaskError::internal("decode_nonterminal", error))
    }

    pub(super) async fn insert_or_get(
        &self,
        request: &CreateTaskRequest,
        request_hash: &str,
        now_epoch_ms: i64,
    ) -> Result<InsertOutcome, TaskError> {
        let operation = request.operation.as_ref().expect("validated operation");
        let encoded = request.encode_to_vec();
        let inserted = base_db::sqlx::query(
            "INSERT OR IGNORE INTO avai_task(task_id,idempotency_key,request_hash,request,capability,route_id,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?)",
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
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("insert_task", error))?;
        if inserted.rows_affected() == 0 {
            let existing = if let Some(existing) = self.get(&request.task_id).await? {
                Some(existing)
            } else {
                base_db::sqlx::query(SELECT_TASK_BY_IDEMPOTENCY)
                    .bind(&operation.idempotency_key)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| TaskError::internal("query_existing", error))?
                    .map(decode_task_row)
                    .transpose()?
            };
            return existing
                .map(|existing| existing_outcome(existing, operation, request_hash))
                .unwrap_or_else(|| {
                    Err(TaskError::internal(
                        "resolve_insert_conflict",
                        "ignored task insert has no conflicting row",
                    ))
                });
        }
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
                execution_binding: None,
                result: None,
                error_code: None,
                error_message: None,
            },
        })
    }

    pub(super) async fn get(&self, task_id: &str) -> Result<Option<TaskRecord>, TaskError> {
        base_db::sqlx::query(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| TaskError::internal("query_task", error))?
            .map(decode_task_row)
            .transpose()
    }

    pub(super) async fn list(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let rows = base_db::sqlx::query(SELECT_ALL_TASKS)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| TaskError::internal("list_tasks", error))?;
        rows.into_iter().map(decode_task_row).collect()
    }

    pub(super) async fn claim(
        &self,
        task_id: &str,
        execution_binding: &[u8],
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let updated = base_db::sqlx::query(
            "UPDATE avai_task SET state=?, execution_binding=?, updated_at_ms=? WHERE task_id=? AND state=? AND (execution_binding IS NULL OR execution_binding=?)",
        )
        .bind(AiTaskState::Running as i32)
        .bind(execution_binding)
        .bind(now_ms)
        .bind(task_id)
        .bind(AiTaskState::Pending as i32)
        .bind(execution_binding)
        .execute(&self.pool)
        .await
        .map_err(|error| TaskError::internal("claim_task", error))?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.get(task_id).await
    }

    pub(super) async fn succeed(
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
        let record = self.get(task_id).await?;
        if let Some(record) = &record {
            self.observe_terminal(record, TaskTerminalOutcome::Succeeded);
        }
        Ok(record)
    }

    pub(super) async fn fail_pending(
        &self,
        task_id: &str,
        code: &str,
        message: &str,
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        self.fail_from_state(task_id, code, message, now_ms, AiTaskState::Pending)
            .await
    }

    pub(super) async fn fail_running(
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
        let record = self.get(task_id).await?;
        if let Some(record) = &record {
            self.observe_terminal(record, TaskTerminalOutcome::Failed);
        }
        Ok(record)
    }

    pub(super) async fn cancel(
        &self,
        task_id: &str,
        now_ms: i64,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let updated = base_db::sqlx::query(
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
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        let record = self.get(task_id).await?;
        if let Some(record) = &record {
            self.observe_terminal(record, TaskTerminalOutcome::Cancelled);
        }
        Ok(record)
    }

    fn observe_terminal(&self, record: &TaskRecord, outcome: TaskTerminalOutcome) {
        let binding = record
            .execution_binding
            .as_deref()
            .and_then(|bytes| ExecutionBinding::decode(bytes).ok());
        let actual_model = binding.as_ref().map(ExecutionBinding::metric_identity);
        self.observability
            .observe_task_terminal(actual_model.clone(), outcome);
        let requested_model = CreateTaskRequest::decode(record.request.as_slice())
            .ok()
            .and_then(|request| request.requested_model)
            .as_ref()
            .map(model_ref_value)
            .unwrap_or_else(|| "default".to_string());
        let actual_model = actual_model
            .as_ref()
            .map(ActualModelIdentity::metric_value)
            .unwrap_or_else(|| "none".to_string());
        base::log::debug!(
            "AVAI task terminal: action=ai_task, stage=terminal, outcome={}, task_id={}, capability={}, requested_model={}, actual_model={}, generation=none, error_code={}",
            terminal_outcome_name(outcome),
            record.task_id,
            record.capability,
            requested_model,
            actual_model,
            record.error_code.as_deref().unwrap_or("none")
        );
    }

    pub(super) async fn close(&self) {
        self.pool.close().await;
    }
}

fn existing_outcome(
    existing: TaskRecord,
    operation: &gmv_protocol::common::v1::OperationRef,
    request_hash: &str,
) -> Result<InsertOutcome, TaskError> {
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

const SELECT_TASK_BY_ID: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,execution_binding,result,error_code,error_message FROM avai_task WHERE task_id=?";
const SELECT_TASK_BY_IDEMPOTENCY: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,execution_binding,result,error_code,error_message FROM avai_task WHERE idempotency_key=?";
const SELECT_ALL_TASKS: &str = "SELECT task_id,idempotency_key,request_hash,request,capability,route_id,state,execution_binding,result,error_code,error_message FROM avai_task ORDER BY created_at_ms,task_id";

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
        execution_binding: row
            .try_get("execution_binding")
            .map_err(|error| TaskError::internal("decode_execution_binding", error))?,
        result,
        error_code: row
            .try_get("error_code")
            .map_err(|error| TaskError::internal("decode_error_code", error))?,
        error_message: row
            .try_get("error_message")
            .map_err(|error| TaskError::internal("decode_error_message", error))?,
    })
}
