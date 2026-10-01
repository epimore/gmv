use super::{
    Arc, AtomicUsize, HEALTH_TIMEOUT, InstalledModel, Instant, LoadFailureEvidence, LoadedModel,
    ModelError, ModelIdentity, ModelInstance, ModelManager, ModelResult, ModelState, Ordering,
    PRELOAD_TIMEOUT, RuntimeCallContext, SELF_TEST_TIMEOUT, UNLOAD_TIMEOUT,
    load_installed_execution_contract,
};

impl ModelManager {
    pub async fn preload(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<()> {
        self.preload_with_context(
            identity,
            now_epoch_ms,
            self.runtime_context(PRELOAD_TIMEOUT),
        )
        .await
    }

    pub async fn preload_with_context(
        &self,
        identity: &ModelIdentity,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<()> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.loaded.lock().await.contains_key(identity) {
            return Ok(());
        }
        let model =
            self.repository.get(identity).await?.ok_or_else(|| {
                ModelError::new("model_not_found", "installed model does not exist")
            })?;
        let provider = self.providers.get(&model.runtime).ok_or_else(|| {
            ModelError::new(
                "model_runtime_unavailable",
                format!("runtime provider is unavailable: {}", model.runtime),
            )
        })?;
        let execution_contract = match load_installed_execution_contract(&model) {
            Ok(contract) => contract,
            Err(error) => {
                self.record_load_failure(identity, &error, now_epoch_ms)
                    .await?;
                return Err(error);
            }
        };
        self.ensure_budget(&model).await?;
        let preload_started = Instant::now();
        let instance = match provider
            .preload(&model, context.with_local_maximum(PRELOAD_TIMEOUT))
            .await
        {
            Ok(instance) => instance,
            Err(error) => {
                let recorded = self
                    .record_load_failure(identity, &error, now_epoch_ms)
                    .await;
                self.observability
                    .observe_preload(preload_started.elapsed());
                base::log::warn!(
                    "Model preload failed: action=model_lifecycle, stage=preload, outcome=failed, model_id={}, version={}, revision={}, runtime={}, error_code={}",
                    identity.model_id,
                    identity.version,
                    identity.revision,
                    model.runtime,
                    error.code
                );
                recorded?;
                return Err(error);
            }
        };
        if let Err(error) = validate_instance(&model, &instance) {
            self.cleanup_rejected_instance(&instance).await;
            let recorded = self
                .record_load_failure(identity, &error, now_epoch_ms)
                .await;
            self.observability
                .observe_preload(preload_started.elapsed());
            recorded?;
            return Err(error);
        }
        if let Err(error) = instance
            .self_test(
                &model.self_tests,
                context.with_local_maximum(SELF_TEST_TIMEOUT),
            )
            .await
        {
            self.cleanup_rejected_instance(&instance).await;
            let recorded = self
                .record_load_failure(identity, &error, now_epoch_ms)
                .await;
            self.observability.observe_self_test_failure();
            self.observability
                .observe_preload(preload_started.elapsed());
            base::log::warn!(
                "Model self-test failed: action=model_lifecycle, stage=self_test, outcome=failed, model_id={}, version={}, revision={}, runtime={}, error_code={}",
                identity.model_id,
                identity.version,
                identity.revision,
                model.runtime,
                error.code
            );
            recorded?;
            return Err(error);
        }
        if let Err(error) = instance
            .health(context.with_local_maximum(HEALTH_TIMEOUT))
            .await
        {
            self.cleanup_rejected_instance(&instance).await;
            let recorded = self
                .record_load_failure(identity, &error, now_epoch_ms)
                .await;
            self.observability
                .observe_preload(preload_started.elapsed());
            recorded?;
            return Err(error);
        }
        if let Err(error) = self
            .repository
            .set_state(identity, ModelState::Ready, None, now_epoch_ms)
            .await
        {
            self.observability
                .observe_preload(preload_started.elapsed());
            base::log::warn!(
                "Model preload failed: action=model_lifecycle, stage=preload, outcome=failed, model_id={}, version={}, revision={}, runtime={}, error_code={}",
                identity.model_id,
                identity.version,
                identity.revision,
                model.runtime,
                error.code
            );
            return Err(error);
        }
        let runtime = model.runtime.clone();
        self.loaded.lock().await.insert(
            identity.clone(),
            Arc::new(LoadedModel {
                model,
                instance,
                result_schema: execution_contract.result_schema,
                in_flight: AtomicUsize::new(0),
            }),
        );
        self.refresh_ready_models().await;
        self.observability
            .observe_preload(preload_started.elapsed());
        base::log::info!(
            "Model preload succeeded: action=model_lifecycle, stage=preload, outcome=succeeded, model_id={}, version={}, revision={}, runtime={}",
            identity.model_id,
            identity.version,
            identity.revision,
            runtime
        );
        Ok(())
    }

    pub async fn unload(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<()> {
        self.unload_with_context(identity, now_epoch_ms, self.runtime_context(UNLOAD_TIMEOUT))
            .await
    }

    pub async fn unload_with_context(
        &self,
        identity: &ModelIdentity,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<()> {
        let _lifecycle = self.lifecycle.lock().await;
        let referenced_by_slot = self.slots.read().await.values().any(|slot| {
            slot.active
                .as_ref()
                .is_some_and(|value| value.loaded.model.identity == *identity)
                || slot
                    .previous
                    .as_ref()
                    .is_some_and(|value| value.loaded.model.identity == *identity)
        });
        if referenced_by_slot {
            return Err(ModelError::new(
                "model_in_use",
                "active or previous model cannot be unloaded",
            ));
        }
        let Some(candidate) = self.loaded.lock().await.get(identity).cloned() else {
            return Ok(());
        };
        if candidate.in_flight.load(Ordering::Acquire) != 0 {
            return Err(ModelError::new(
                "model_in_use",
                "model still has in-flight tasks",
            ));
        }
        candidate
            .instance
            .unload(context.with_local_maximum(UNLOAD_TIMEOUT))
            .await?;
        self.repository
            .set_state(identity, ModelState::Installed, None, now_epoch_ms)
            .await?;
        self.loaded.lock().await.remove(identity);
        self.refresh_ready_models().await;
        base::log::info!(
            "Model unload completed: action=model_lifecycle, stage=unload, outcome=succeeded, model_id={}, version={}, revision={}",
            identity.model_id,
            identity.version,
            identity.revision
        );
        Ok(())
    }

    pub(super) async fn cleanup_rejected_instance(&self, instance: &Arc<dyn ModelInstance>) {
        if let Err(error) = instance.unload(self.runtime_context(UNLOAD_TIMEOUT)).await {
            base::log::warn!(
                "model instance cleanup failed: action=unload_rejected_instance, error_code={}",
                error.code
            );
        }
    }

    pub(super) async fn record_load_failure(
        &self,
        identity: &ModelIdentity,
        error: &ModelError,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        if classify_load_failure(error) == LoadFailureEvidence::IntrinsicModel {
            self.repository
                .set_state(identity, ModelState::Failed, None, now_epoch_ms)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn ensure_budget(&self, model: &InstalledModel) -> ModelResult<()> {
        let loaded = self.loaded.lock().await;
        let memory_mb = loaded
            .values()
            .map(|loaded| loaded.model.resources.memory_mb)
            .sum::<u64>();
        let vram_mb = loaded
            .values()
            .map(|loaded| loaded.model.resources.vram_mb)
            .sum::<u64>();
        if loaded.len() >= self.config.max_loaded_models
            || memory_mb.saturating_add(model.resources.memory_mb) > self.config.max_memory_mb
            || vram_mb.saturating_add(model.resources.vram_mb) > self.config.max_vram_mb
        {
            return Err(ModelError::new(
                "model_runtime_budget_exceeded",
                "preloading this model would exceed the loaded model budget",
            ));
        }
        Ok(())
    }
}

pub(super) fn classify_load_failure(error: &ModelError) -> LoadFailureEvidence {
    match error.code {
        "invalid_model_runtime_config"
        | "model_runtime_unavailable"
        | "model_runtime_provider_unavailable"
        | "model_runtime_protocol_mismatch"
        | "model_runtime_protocol_violation"
        | "model_runtime_stale_handle"
        | "model_runtime_busy"
        | "model_runtime_deadline_exceeded"
        | "model_runtime_cancelled"
        | "model_runtime_response_invalid" => LoadFailureEvidence::RuntimeAvailability,
        _ => LoadFailureEvidence::IntrinsicModel,
    }
}

pub(super) fn validate_instance(
    model: &InstalledModel,
    instance: &Arc<dyn ModelInstance>,
) -> ModelResult<()> {
    if instance.identity() != &model.identity
        || instance.runtime() != model.runtime
        || instance.capabilities() != model.capabilities
    {
        return Err(ModelError::new(
            "model_runtime_contract_mismatch",
            "runtime instance metadata does not match the installed model",
        ));
    }
    Ok(())
}
