use super::*;

pub(super) struct WorkerContext {
    pub(super) identity: NodeIdentity,
    pub(super) repository: TaskRepository,
    pub(super) feedback: Option<Arc<FeedbackManager>>,
    pub(super) resolver: SourceResolver,
    pub(super) provider: Arc<ProviderRegistry>,
    pub(super) receiver: Arc<Mutex<mpsc::Receiver<String>>>,
    pub(super) cancel: CancellationToken,
    pub(super) task_cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub(super) event_sender: Arc<RwLock<Option<NodeEventSender>>>,
    pub(super) running: Arc<AtomicUsize>,
}

pub(super) async fn worker_loop(context: WorkerContext) {
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
        let result = process_task(&context, &task_id).await;
        context.running.fetch_sub(1, Ordering::AcqRel);
        match result {
            Ok(Some(record)) => emit_terminal_event(&context, &record).await,
            Ok(None) => {}
            Err(error) => {
                base::log::error!(
                    "Avai task execution failed: action=ai_task, stage=worker, task_id={}, error_code={}, error={}",
                    task_id,
                    error.code,
                    error.message
                );
            }
        }
    }
}

async fn process_task(
    context: &WorkerContext,
    task_id: &str,
) -> Result<Option<TaskRecord>, TaskError> {
    let Some(record) = context.repository.get(task_id).await? else {
        return Ok(None);
    };
    if record.state != AiTaskState::Pending {
        return Ok(None);
    }
    let request = CreateTaskRequest::decode(record.request.as_slice())
        .map_err(|error| TaskError::internal("decode_request", error))?;
    let selected = context.feedback.as_ref().is_some_and(|feedback| {
        sampled(
            feedback.config(),
            &record.capability,
            task_id,
            &record.request_hash,
        )
    });
    if request.deadline_epoch_ms != 0 && request.deadline_epoch_ms <= now_epoch_ms() {
        return context
            .repository
            .fail_pending(
                task_id,
                "task_expired",
                "task deadline expired while waiting for execution",
                now_epoch_ms(),
            )
            .await;
    }
    let durable_binding = record
        .execution_binding
        .as_deref()
        .map(ExecutionBinding::decode)
        .transpose();
    let durable_binding = match durable_binding {
        Ok(binding) => binding,
        Err(error) => {
            return context
                .repository
                .fail_pending(task_id, error.code, &error.message, now_epoch_ms())
                .await;
        }
    };
    let capture = context
        .provider
        .capture(
            &record.capability,
            request.requested_model.as_ref(),
            durable_binding.as_ref(),
        )
        .await;
    let captured = match capture {
        Ok(captured) => captured,
        Err(error) => {
            return context
                .repository
                .fail_pending(task_id, error.code, &error.message, now_epoch_ms())
                .await;
        }
    };
    let binding = captured.binding().encode()?;
    let Some(_claimed) = context
        .repository
        .claim(task_id, &binding, now_epoch_ms())
        .await?
    else {
        return Ok(None);
    };
    base::log::debug!(
        "AVAI task dispatched: action=ai_task, stage=dispatch, outcome=succeeded, task_id={}, capability={}, requested_model={}, actual_model={}, generation={}",
        task_id,
        record.capability,
        request
            .requested_model
            .as_ref()
            .map(model_ref_value)
            .unwrap_or_else(|| "default".to_string()),
        captured.binding().metric_identity().metric_value(),
        captured
            .generation()
            .map_or_else(|| "none".to_string(), |value| value.to_string())
    );
    let task_cancel = CancellationToken::new();
    context
        .task_cancellations
        .lock()
        .await
        .insert(task_id.to_string(), task_cancel.clone());
    match context.repository.get(task_id).await {
        Ok(Some(current)) if current.state != AiTaskState::Running => {
            context.task_cancellations.lock().await.remove(task_id);
            return Ok(Some(current));
        }
        Ok(_) => {}
        Err(error) => {
            context.task_cancellations.lock().await.remove(task_id);
            return Err(error);
        }
    }
    let source = request
        .source
        .as_ref()
        .ok_or_else(|| TaskError::new("invalid_source", "persisted task has no typed source"));
    let resolve = async {
        match source {
            Ok(source) => context
                .resolver
                .resolve(source, &record.capability, now_epoch_ms())
                .await
                .map_err(source_task_error),
            Err(error) => Err(error),
        }
    };
    base::tokio::pin!(resolve);
    let image = base::tokio::select! {
        _ = context.cancel.cancelled() => {
            context.task_cancellations.lock().await.remove(task_id);
            return Ok(None);
        },
        _ = task_cancel.cancelled() => {
            context.task_cancellations.lock().await.remove(task_id);
            return context.repository.get(task_id).await.map(|record| {
                record.filter(|record| record.state != AiTaskState::Running)
            });
        },
        _ = task_deadline(request.deadline_epoch_ms) => {
            context.task_cancellations.lock().await.remove(task_id);
            return context.repository.fail_running(
                task_id,
                "task_expired",
                "task deadline expired during source resolution",
                now_epoch_ms(),
            ).await;
        },
        image = &mut resolve => image,
    };
    let image = match image {
        Ok(image) => image,
        Err(error) => {
            context.task_cancellations.lock().await.remove(task_id);
            return context
                .repository
                .fail_running(task_id, error.code, &error.message, now_epoch_ms())
                .await;
        }
    };
    let evidence = selected.then(|| {
        (
            image.bytes.clone(),
            image.sha256.clone(),
            image.content_type.clone(),
        )
    });
    let source_ref = if selected {
        request.source.as_ref().and_then(safe_source_ref)
    } else {
        None
    };
    if selected && source_ref.is_none() {
        base::log::warn!("AVAI feedback skipped: action=feedback, reason=unsafe_source_ref");
    }
    let runtime_cancel = CancellationToken::new();
    let runtime_context = RuntimeCallContext {
        deadline: runtime_deadline(request.deadline_epoch_ms),
        cancellation: runtime_cancel.clone(),
    };
    let inference = captured.infer(&record.capability, image, runtime_context);
    base::tokio::pin!(inference);
    let (terminal, drain_native) = base::tokio::select! {
        _ = context.cancel.cancelled() => {
            runtime_cancel.cancel();
            (Ok(None), true)
        },
        _ = task_cancel.cancelled() => {
            runtime_cancel.cancel();
            (context.repository.get(task_id).await.map(|record| {
                record.filter(|record| record.state != AiTaskState::Running)
            }), true)
        },
        _ = task_deadline(request.deadline_epoch_ms) => {
            runtime_cancel.cancel();
            (context.repository.fail_running(
                task_id,
                "task_expired",
                "task deadline expired during execution",
                now_epoch_ms(),
            ).await, true)
        },
        output = &mut inference => {
            let terminal = match output {
                Ok(output) => {
                    let prepared = if let (Some(feedback), Some((bytes, sha, media_type)), Some(source_ref)) =
                        (&context.feedback, &evidence, &source_ref)
                    {
                        let actual = output.result.actual_model.as_ref();
                        let schema = output.result.output.as_ref();
                        let binding = captured.binding();
                        if actual.is_some_and(|model| model.model_id == binding.model_id
                            && model.version == binding.model_version
                            && model.revision == binding.revision && model.runtime == binding.runtime)
                            && schema.is_some_and(|value| value.schema == binding.result_schema_name
                                && value.version == binding.result_schema_version)
                        {
                            let material = FeedbackMaterial {
                                task_id: task_id.to_string(), request_hash: record.request_hash.clone(),
                                route_id: record.route_id.clone(), capability: record.capability.clone(),
                                source_ref: source_ref.clone(), result: output.result.clone(),
                                evidence: bytes.clone(), evidence_sha256: sha.clone(),
                                evidence_media_type: media_type.clone(),
                            };
                            match feedback.prepare(material).await {
                                Ok(id) => id,
                                Err(error) => { base::log::warn!("AVAI feedback skipped: action=feedback, reason={}", error.code); None },
                            }
                        } else {
                            base::log::warn!("AVAI feedback skipped: action=feedback, reason=result_binding_mismatch");
                            None
                        }
                    } else { None };
                    let terminal = context.repository.succeed(task_id, output, now_epoch_ms()).await;
                    if let (Some(feedback), Some(id)) = (&context.feedback, prepared) {
                        if matches!(&terminal, Ok(Some(record)) if record.state == AiTaskState::Succeeded) {
                            if let Err(error) = feedback.promote(&id).await {
                                base::log::warn!("AVAI feedback promotion deferred: action=feedback, reason={}", error.code);
                            }
                        } else if let Err(error) = feedback.discard(&id).await {
                            base::log::warn!("AVAI feedback discard deferred: action=feedback, reason={}", error.code);
                        }
                    }
                    terminal
                },
                Err(error) => context.repository.fail_running(
                    task_id,
                    error.code,
                    &error.message,
                    now_epoch_ms(),
                ).await,
            };
            (terminal, false)
        },
    };
    if drain_native {
        let _late_result = inference.await;
    }
    context.task_cancellations.lock().await.remove(task_id);
    terminal
}

async fn task_deadline(deadline_epoch_ms: i64) {
    if deadline_epoch_ms == 0 {
        std::future::pending::<()>().await;
    } else {
        let remaining = deadline_epoch_ms.saturating_sub(now_epoch_ms());
        base::tokio::time::sleep(Duration::from_millis(
            u64::try_from(remaining.max(0)).unwrap_or_default(),
        ))
        .await;
    }
}

fn runtime_deadline(deadline_epoch_ms: i64) -> Instant {
    let local = Instant::now() + MANAGED_INFERENCE_TIMEOUT;
    if deadline_epoch_ms == 0 {
        return local;
    }
    let external = Instant::now()
        + Duration::from_millis(
            u64::try_from(deadline_epoch_ms.saturating_sub(now_epoch_ms()).max(0))
                .unwrap_or_default(),
        );
    local.min(external)
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
