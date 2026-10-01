use super::*;

impl TaskManager {
    pub async fn open(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: TaskManagerConfig,
        runtime: &GlobalRuntime,
    ) -> Result<Self, TaskError> {
        Self::open_with_model_manager(identity, capabilities, config, None, runtime).await
    }

    pub async fn open_with_model_manager(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: TaskManagerConfig,
        model_manager: Option<ModelManager>,
        runtime: &GlobalRuntime,
    ) -> Result<Self, TaskError> {
        Self::open_with_model_manager_and_observability(
            identity,
            capabilities,
            config,
            model_manager,
            runtime,
            Arc::new(Observability::new()),
        )
        .await
    }

    pub async fn open_with_model_manager_and_observability(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: TaskManagerConfig,
        model_manager: Option<ModelManager>,
        runtime: &GlobalRuntime,
        observability: Arc<Observability>,
    ) -> Result<Self, TaskError> {
        Self::open_with_feedback(
            identity,
            capabilities,
            config,
            model_manager,
            runtime,
            observability,
            None,
        )
        .await
    }

    pub async fn open_with_feedback(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: TaskManagerConfig,
        model_manager: Option<ModelManager>,
        runtime: &GlobalRuntime,
        observability: Arc<Observability>,
        feedback_config: Option<(FeedbackConfig, String, String)>,
    ) -> Result<Self, TaskError> {
        if config.queue_size == 0 || config.worker_count == 0 || config.max_result_bytes == 0 {
            return Err(TaskError::new(
                "invalid_task_config",
                "queue_size, worker_count and max_result_bytes must be greater than zero",
            ));
        }
        let repository =
            TaskRepository::open_with_observability(&config.database_path, observability).await?;
        repository.recover_interrupted().await?;
        let feedback = if let Some((settings, installation_id, host_id)) = feedback_config {
            match FeedbackManager::open(
                &config.database_path,
                settings,
                installation_id,
                host_id,
                identity.clone(),
            )
            .await
            {
                Ok(manager) => Some(Arc::new(manager)),
                Err(error) => {
                    base::log::warn!(
                        "AVAI feedback disabled: action=feedback, reason={}",
                        error.code
                    );
                    None
                }
            }
        } else {
            None
        };
        let pending = repository.pending_task_ids().await?;
        let resolver = SourceResolver::new(identity.clone(), config.source_policy, runtime)
            .map_err(source_task_error)?;
        let provider = Arc::new(ProviderRegistry::new(
            &capabilities,
            model_manager,
            config.max_result_bytes,
        )?);
        let capabilities = Arc::new(capabilities.into_iter().collect::<HashSet<_>>());
        let (queue, receiver) = mpsc::channel(config.queue_size);
        let receiver = Arc::new(Mutex::new(receiver));
        let cancel = runtime.cancel.child_token();
        let workers = Arc::new(Mutex::new(Vec::new()));
        let task_cancellations = Arc::new(Mutex::new(HashMap::new()));
        let event_sender = Arc::new(RwLock::new(None));
        let running = Arc::new(AtomicUsize::new(0));
        let runtime_health = ComponentRuntimeHealth::default();

        if let Some(feedback) = feedback.clone() {
            let sweep_cancel = cancel.clone();
            let handle = runtime.spawn("avai-feedback-expiry", async move {
                let mut interval = base::tokio::time::interval(Duration::from_secs(60));
                interval.tick().await;
                loop {
                    base::tokio::select! {
                        _ = sweep_cancel.cancelled() => break,
                        _ = interval.tick() => {
                            if let Err(error) = feedback.sweep().await {
                                base::log::warn!("AVAI feedback expiry deferred: action=feedback, reason={}", error.code);
                            }
                        }
                    }
                }
            }).map_err(|error| TaskError::internal("spawn_feedback_expiry", error))?;
            workers.lock().await.push(handle);
        }
        for worker_id in 0..config.worker_count {
            let context = WorkerContext {
                identity: identity.clone(),
                repository: repository.clone(),
                feedback: feedback.clone(),
                resolver: resolver.clone(),
                provider: provider.clone(),
                receiver: receiver.clone(),
                cancel: cancel.clone(),
                task_cancellations: task_cancellations.clone(),
                event_sender: event_sender.clone(),
                running: running.clone(),
            };
            let critical = runtime_health.register_critical();
            let handle = runtime
                .spawn(format!("avai-task-worker-{worker_id}"), async move {
                    worker_loop(context).await;
                    drop(critical);
                })
                .map_err(|error| TaskError::internal("spawn_worker", error))?;
            workers.lock().await.push(handle);
        }

        let manager = Self {
            identity,
            capabilities,
            repository,
            feedback,
            queue,
            cancel,
            workers,
            task_cancellations,
            event_sender,
            running,
            closed: Arc::new(AtomicBool::new(false)),
            admission: AdmissionBarrier::default(),
            runtime_health,
            #[cfg(test)]
            admission_pause: Arc::new(std::sync::Mutex::new(None)),
        };
        if !pending.is_empty() {
            let recovery_queue = manager.queue.clone();
            let recovery_cancel = manager.cancel.clone();
            let recovery = runtime
                .spawn("avai-task-recovery-feeder", async move {
                    for task_id in pending {
                        let sent = base::tokio::select! {
                            _ = recovery_cancel.cancelled() => return,
                            sent = recovery_queue.send(task_id) => sent,
                        };
                        if sent.is_err() {
                            base::log::error!(
                                "Avai task recovery stopped: action=ai_task, stage=recovery_enqueue, reason=worker_queue_closed"
                            );
                            GlobalRuntime::request_shutdown_with_error();
                            return;
                        }
                    }
                })
                .map_err(|error| TaskError::internal("spawn_recovery_feeder", error))?;
            manager.workers.lock().await.push(recovery);
        }
        Ok(manager)
    }

    pub fn feedback_manager(&self) -> Option<FeedbackManager> {
        self.feedback
            .as_ref()
            .map(|manager| manager.as_ref().clone())
    }

    pub async fn set_event_sender(&self, sender: NodeEventSender) {
        *self.event_sender.write().await = Some(sender);
    }

    pub fn running_task_count(&self) -> usize {
        self.running.load(Ordering::Acquire)
    }

    pub fn mark_runtime_ready(&self) {
        self.runtime_health.mark_ready();
    }

    pub fn close_upgrade_admission(&self) {
        self.admission.close();
    }

    pub fn reopen_upgrade_admission(&self) {
        self.admission.reopen();
    }

    pub fn in_flight_upgrade_admissions(&self) -> usize {
        self.admission.in_flight()
    }

    pub async fn wait_for_upgrade_admissions(&self) {
        self.admission.wait_for_zero().await;
    }

    pub async fn is_upgrade_drained(&self) -> Result<bool, TaskError> {
        Ok(self.admission.in_flight() == 0 && self.repository.nonterminal_task_count().await? == 0)
    }

    pub async fn durable_nonterminal_task_count(&self) -> Result<usize, TaskError> {
        self.repository.nonterminal_task_count().await
    }

    #[cfg(test)]
    pub(crate) async fn insert_nonterminal_task_for_test(
        &self,
        task_id: &str,
    ) -> Result<(), TaskError> {
        self.repository
            .insert_nonterminal_task_for_test(task_id)
            .await
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
        let Some(_permit) = self.admission.acquire() else {
            return Err(TaskError::new(
                "component_draining",
                "Avai is draining for upgrade",
            ));
        };
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskError::new(
                "executor_unavailable",
                "Avai task manager is stopping",
            ));
        }
        #[cfg(test)]
        self.pause_after_admission().await;
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
        let outcome = self
            .repository
            .cancel(&request.task_id, now_epoch_ms())
            .await;
        if outcome.as_ref().is_ok_and(|record| {
            record
                .as_ref()
                .is_some_and(|record| record.state == AiTaskState::Cancelled)
        }) && let Some(cancel) = self
            .task_cancellations
            .lock()
            .await
            .remove(&request.task_id)
        {
            cancel.cancel();
        }
        match outcome {
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
        self.admission.close();
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

    #[cfg(test)]
    async fn pause_after_admission(&self) {
        let pause = self
            .admission_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(pause) = pause {
            pause.entered.add_permits(1);
            pause.release.acquire().await.unwrap().forget();
        }
    }
}

#[tonic::async_trait]
impl ComponentDrainBehavior for AvaiDrainBehavior {
    fn probe_snapshot(&self) -> ComponentProbeSnapshot {
        self.0.runtime_health.snapshot(
            self.0.admission.is_accepting()
                && !self.0.closed.load(Ordering::Acquire)
                && !self.0.cancel.is_cancelled(),
        )
    }

    fn supported(&self) -> bool {
        true
    }

    fn close_admission(&self) {
        self.0.close_upgrade_admission();
    }

    async fn reopen_admission(&self) -> Result<(), &'static str> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err("avai_executor_unavailable");
        }
        self.0.reopen_upgrade_admission();
        Ok(())
    }

    fn in_flight_admissions(&self) -> usize {
        self.0.in_flight_upgrade_admissions()
    }

    async fn wait_for_admissions(&self) {
        self.0.wait_for_upgrade_admissions().await;
    }

    async fn is_drained(&self) -> bool {
        self.0.is_upgrade_drained().await.unwrap_or(false)
    }

    async fn drain_owned_resources(&self, cancel: CancellationToken) -> Result<(), &'static str> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            match self.0.is_upgrade_drained().await {
                Ok(true) => return Ok(()),
                Ok(false) => base::tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                Err(_) => return Err("avai_drain_state_unavailable"),
            }
        }
    }
}
