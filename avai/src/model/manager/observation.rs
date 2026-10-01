use super::{
    Arc, CapabilityGeneration, Duration, HEALTH_TIMEOUT, HashSet, ModelError, ModelIdentity,
    ModelManager, ModelObservation, ModelResult, ModelState, ModelStatus, Ordering,
    RuntimeCallContext, RuntimeVariant,
};

impl ModelManager {
    pub async fn status(&self) -> Vec<ModelStatus> {
        let loaded = self.loaded.lock().await;
        let slots = self.slots.read().await;
        loaded
            .values()
            .map(|loaded| {
                let mut active_capabilities = slots
                    .iter()
                    .filter_map(|(capability, slot)| {
                        slot.active
                            .as_ref()
                            .filter(|active| Arc::ptr_eq(&active.loaded, loaded))
                            .map(|_| capability.clone())
                    })
                    .collect::<Vec<_>>();
                active_capabilities.sort();
                let generation = slots.values().find_map(|slot| {
                    slot.active
                        .as_ref()
                        .filter(|active| Arc::ptr_eq(&active.loaded, loaded))
                        .map(|active| active.generation)
                });
                ModelStatus {
                    identity: loaded.model.identity.clone(),
                    runtime: loaded.model.runtime.clone(),
                    generation,
                    active_capabilities,
                    in_flight_tasks: loaded.in_flight.load(Ordering::Acquire),
                }
            })
            .collect()
    }

    pub async fn observation(&self, identity: &ModelIdentity) -> ModelObservation {
        let loaded = self.loaded.lock().await;
        let slots = self.slots.read().await;
        let candidate = loaded.get(identity);
        let mut active_capabilities = Vec::new();
        let mut previous_capabilities = Vec::new();
        let mut active_bindings = Vec::new();
        let mut previous_bindings = Vec::new();
        let mut generation = None;
        for (capability, slot) in slots.iter() {
            if let Some(active) = &slot.active
                && active.loaded.model.identity == *identity
            {
                active_capabilities.push(capability.clone());
                active_bindings.push(CapabilityGeneration {
                    capability: capability.clone(),
                    generation: active.generation,
                });
                generation = Some(active.generation);
            }
            if let Some(previous) = &slot.previous
                && previous.loaded.model.identity == *identity
            {
                previous_capabilities.push(capability.clone());
                previous_bindings.push(CapabilityGeneration {
                    capability: capability.clone(),
                    generation: previous.generation,
                });
            }
        }
        active_capabilities.sort();
        previous_capabilities.sort();
        active_bindings.sort_by(|left, right| left.capability.cmp(&right.capability));
        previous_bindings.sort_by(|left, right| left.capability.cmp(&right.capability));
        ModelObservation {
            loaded: candidate.is_some(),
            runtime_available: candidate.map_or_else(
                || false,
                |loaded| self.providers.contains_key(&loaded.model.runtime),
            ),
            active_capabilities,
            previous_capabilities,
            active_bindings,
            previous_bindings,
            generation,
            in_flight_tasks: candidate
                .map(|loaded| loaded.in_flight.load(Ordering::Acquire))
                .unwrap_or_default(),
        }
    }

    pub fn runtime_available(&self, runtime: &str) -> bool {
        self.providers.contains_key(runtime)
    }

    pub fn validate_selector(&self, selector: &RuntimeVariant) -> ModelResult<()> {
        self.providers
            .get(&selector.runtime)
            .ok_or_else(|| {
                ModelError::new(
                    "model_runtime_unavailable",
                    "requested runtime provider is not registered",
                )
            })?
            .validate_selector(selector)
    }

    pub async fn health(&self, identity: &ModelIdentity) -> ModelResult<()> {
        self.health_with_context(identity, self.runtime_context(HEALTH_TIMEOUT))
            .await
    }

    pub async fn health_with_context(
        &self,
        identity: &ModelIdentity,
        context: RuntimeCallContext,
    ) -> ModelResult<()> {
        let loaded = self
            .loaded
            .lock()
            .await
            .get(identity)
            .cloned()
            .ok_or_else(|| ModelError::new("model_not_ready", "model is not loaded"))?;
        loaded
            .instance
            .health(context.with_local_maximum(HEALTH_TIMEOUT))
            .await
    }

    pub(super) fn runtime_context(&self, maximum: Duration) -> RuntimeCallContext {
        RuntimeCallContext::local(maximum, self.runtime_cancellation.clone())
    }

    pub(super) async fn refresh_ready_models(&self) {
        let loaded = self.loaded.lock().await.keys().cloned().collect::<Vec<_>>();
        let mut ready = 0;
        for identity in loaded {
            match self.repository.get(&identity).await {
                Ok(Some(model))
                    if matches!(model.state, ModelState::Ready | ModelState::Active) =>
                {
                    ready += 1;
                }
                Ok(_) => {}
                Err(error) => {
                    base::log::warn!(
                        "Ready-model telemetry refresh failed: action=telemetry_refresh, stage=ready_models, outcome=failed, error_code={}",
                        error.code
                    );
                    return;
                }
            }
        }
        self.observability.set_ready_models(ready);
    }

    pub async fn active_identities(&self) -> HashSet<ModelIdentity> {
        self.slots
            .read()
            .await
            .values()
            .filter_map(|slot| {
                slot.active
                    .as_ref()
                    .map(|active| active.loaded.model.identity.clone())
            })
            .collect()
    }
}
