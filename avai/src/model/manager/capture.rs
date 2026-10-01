use super::{
    ActiveModel, Arc, AtomicUsize, HEALTH_TIMEOUT, InferenceResult, InstalledModel, Instant,
    LoadedModel, ModelError, ModelGeneration, ModelIdentity, ModelManager, ModelResult, ModelState,
    Ordering, PRELOAD_TIMEOUT, ResultSchema, RuntimeCallContext, RuntimeInput, SELF_TEST_TIMEOUT,
    load_installed_execution_contract,
};

use super::lifecycle::validate_instance;

impl ActiveModel {
    pub fn identity(&self) -> &ModelIdentity {
        &self.generation.loaded.model.identity
    }

    pub fn generation(&self) -> u64 {
        self.generation.generation
    }

    pub fn runtime(&self) -> &str {
        &self.generation.loaded.model.runtime
    }

    pub fn result_schema(&self) -> &ResultSchema {
        &self.generation.loaded.result_schema
    }

    pub async fn infer(
        &self,
        input: RuntimeInput,
        context: RuntimeCallContext,
    ) -> ModelResult<InferenceResult> {
        self.generation.loaded.instance.infer(input, context).await
    }
}

impl Clone for ActiveModel {
    fn clone(&self) -> Self {
        self.generation
            .loaded
            .in_flight
            .fetch_add(1, Ordering::AcqRel);
        Self {
            generation: self.generation.clone(),
        }
    }
}

impl Drop for ActiveModel {
    fn drop(&mut self) {
        self.generation
            .loaded
            .in_flight
            .fetch_sub(1, Ordering::AcqRel);
    }
}

impl ModelManager {
    pub async fn capture(&self, capability: &str) -> ModelResult<ActiveModel> {
        let generation = self
            .slots
            .read()
            .await
            .get(capability)
            .and_then(|slot| slot.active.clone())
            .ok_or_else(|| ModelError::new("model_not_active", "capability has no active model"))?;
        generation.loaded.in_flight.fetch_add(1, Ordering::AcqRel);
        Ok(ActiveModel { generation })
    }

    pub async fn has_active(&self, capability: &str) -> bool {
        self.slots
            .read()
            .await
            .get(capability)
            .and_then(|slot| slot.active.as_ref())
            .is_some()
    }

    pub async fn capture_exact(
        &self,
        capability: &str,
        identity: &ModelIdentity,
        runtime: &str,
    ) -> ModelResult<ActiveModel> {
        let _lifecycle = self.lifecycle.lock().await;
        let model = self.repository.get(identity).await?.ok_or_else(|| {
            ModelError::new("model_not_found", "installed model revision does not exist")
        })?;
        validate_capture_request(&model, capability, runtime)?;
        match model.state {
            ModelState::Failed => {
                return Err(ModelError::new(
                    "model_failed",
                    "requested model has failed",
                ));
            }
            ModelState::Installed => {
                return Err(ModelError::new(
                    "model_not_ready",
                    "requested model is installed but not loaded",
                ));
            }
            ModelState::Ready | ModelState::Active => {}
            ModelState::Retired => {
                return Err(ModelError::new(
                    "model_not_ready",
                    "requested model is not available for execution",
                ));
            }
        }
        let loaded = self
            .loaded
            .lock()
            .await
            .get(identity)
            .cloned()
            .ok_or_else(|| ModelError::new("model_not_ready", "requested model is not loaded"))?;
        loaded.in_flight.fetch_add(1, Ordering::AcqRel);
        Ok(ActiveModel {
            generation: Arc::new(ModelGeneration {
                generation: model.active_generation.unwrap_or_default(),
                loaded,
            }),
        })
    }

    pub async fn capture_recovered(
        &self,
        capability: &str,
        identity: &ModelIdentity,
        runtime: &str,
        result_schema_name: &str,
        result_schema_version: u32,
        now_epoch_ms: i64,
    ) -> ModelResult<ActiveModel> {
        let _lifecycle = self.lifecycle.lock().await;
        let model = self.repository.get(identity).await?.ok_or_else(|| {
            ModelError::new(
                "bound_model_unavailable",
                "bound model revision does not exist",
            )
        })?;
        validate_capture_request(&model, capability, runtime)?;
        if matches!(model.state, ModelState::Failed | ModelState::Retired) {
            return Err(ModelError::new(
                "bound_model_unavailable",
                "bound model is not recoverable",
            ));
        }
        let loaded = if let Some(loaded) = self.loaded.lock().await.get(identity).cloned() {
            loaded
        } else {
            let provider = self.providers.get(&model.runtime).ok_or_else(|| {
                ModelError::new(
                    "bound_model_unavailable",
                    "bound model runtime is unavailable",
                )
            })?;
            self.ensure_budget(&model).await?;
            let execution_contract = match load_installed_execution_contract(&model) {
                Ok(contract) => contract,
                Err(error) => {
                    self.record_load_failure(identity, &error, now_epoch_ms)
                        .await?;
                    return Err(error);
                }
            };
            let preload_started = Instant::now();
            let instance = match provider
                .preload(&model, self.runtime_context(PRELOAD_TIMEOUT))
                .await
            {
                Ok(instance) => instance,
                Err(error) => {
                    let recorded = self
                        .record_load_failure(identity, &error, now_epoch_ms)
                        .await;
                    self.observability
                        .observe_preload(preload_started.elapsed());
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
                .self_test(&model.self_tests, self.runtime_context(SELF_TEST_TIMEOUT))
                .await
            {
                self.cleanup_rejected_instance(&instance).await;
                let recorded = self
                    .record_load_failure(identity, &error, now_epoch_ms)
                    .await;
                self.observability.observe_self_test_failure();
                self.observability
                    .observe_preload(preload_started.elapsed());
                recorded?;
                return Err(error);
            }
            if let Err(error) = instance.health(self.runtime_context(HEALTH_TIMEOUT)).await {
                self.cleanup_rejected_instance(&instance).await;
                let recorded = self
                    .record_load_failure(identity, &error, now_epoch_ms)
                    .await;
                self.observability
                    .observe_preload(preload_started.elapsed());
                recorded?;
                return Err(error);
            }
            if model.state == ModelState::Installed
                && let Err(error) = self
                    .repository
                    .set_state(identity, ModelState::Ready, None, now_epoch_ms)
                    .await
            {
                self.observability
                    .observe_preload(preload_started.elapsed());
                return Err(error);
            }
            let loaded = Arc::new(LoadedModel {
                model: model.clone(),
                instance,
                result_schema: execution_contract.result_schema,
                in_flight: AtomicUsize::new(0),
            });
            self.loaded
                .lock()
                .await
                .insert(identity.clone(), loaded.clone());
            self.refresh_ready_models().await;
            self.observability
                .observe_preload(preload_started.elapsed());
            loaded
        };
        if loaded.result_schema.name != result_schema_name
            || loaded.result_schema.version != result_schema_version
        {
            return Err(ModelError::new(
                "bound_model_unavailable",
                "bound model result contract no longer matches",
            ));
        }
        loaded.in_flight.fetch_add(1, Ordering::AcqRel);
        Ok(ActiveModel {
            generation: Arc::new(ModelGeneration {
                generation: model.active_generation.unwrap_or_default(),
                loaded,
            }),
        })
    }
}

fn validate_capture_request(
    model: &InstalledModel,
    capability: &str,
    runtime: &str,
) -> ModelResult<()> {
    if !model.capabilities.iter().any(|value| value == capability) {
        return Err(ModelError::new(
            "model_capability_incompatible",
            "model does not implement the requested capability",
        ));
    }
    if !runtime.is_empty() && model.runtime != runtime {
        return Err(ModelError::new(
            "model_runtime_incompatible",
            "model runtime does not match the request",
        ));
    }
    Ok(())
}
