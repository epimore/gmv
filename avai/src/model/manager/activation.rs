use super::{
    Arc, CapabilitySlot, HEALTH_TIMEOUT, HashMap, HashSet, ModelError, ModelGeneration,
    ModelIdentity, ModelManager, ModelResult, Ordering, PersistedCapabilitySlot,
    RuntimeCallContext,
};

impl ModelManager {
    pub async fn activate(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<u64> {
        self.activate_with_context(identity, now_epoch_ms, self.runtime_context(HEALTH_TIMEOUT))
            .await
    }

    pub async fn activate_with_context(
        &self,
        identity: &ModelIdentity,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<u64> {
        let _lifecycle = self.lifecycle.lock().await;
        let result = async {
            let loaded = self
                .loaded
                .lock()
                .await
                .get(identity)
                .cloned()
                .ok_or_else(|| {
                    ModelError::new("model_not_ready", "model must be preloaded first")
                })?;
            let slots = self.slots.write().await;
            let declared_capabilities = loaded
                .model
                .capabilities
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let owned_capabilities = slots
                .iter()
                .filter_map(|(capability, slot)| {
                    slot.active
                        .as_ref()
                        .filter(|active| active.loaded.model.identity == *identity)
                        .map(|_| capability.clone())
                })
                .collect::<HashSet<_>>();
            if !owned_capabilities.is_empty() && owned_capabilities != declared_capabilities {
                return Err(ModelError::new(
                    "model_slot_conflict",
                    "model owns only part of its declared capability set",
                ));
            }
            if owned_capabilities == declared_capabilities
                && let Some(existing) = loaded
                    .model
                    .capabilities
                    .first()
                    .and_then(|capability| slots.get(capability))
                    .and_then(|slot| slot.active.as_ref())
            {
                return Ok(existing.generation);
            }
            drop(slots);
            loaded
                .instance
                .health(context.with_local_maximum(HEALTH_TIMEOUT))
                .await?;
            let mut slots = self.slots.write().await;
            let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
            let replaced_capabilities = loaded.model.capabilities.iter().collect::<HashSet<_>>();
            let no_longer_active = slots
                .iter()
                .filter(|(capability, _)| replaced_capabilities.contains(capability))
                .filter_map(|(_, slot)| slot.active.as_ref())
                .filter(|active| {
                    !slots.iter().any(|(capability, slot)| {
                        !replaced_capabilities.contains(capability)
                            && slot.active.as_ref().is_some_and(|candidate| {
                                candidate.loaded.model.identity == active.loaded.model.identity
                            })
                    })
                })
                .map(|active| active.loaded.model.identity.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let persisted_slots = loaded
                .model
                .capabilities
                .iter()
                .map(|capability| {
                    let previous = slots.get(capability).and_then(|slot| slot.active.as_ref());
                    PersistedCapabilitySlot {
                        capability: capability.clone(),
                        active_identity: identity.clone(),
                        active_generation: generation,
                        previous_identity: previous
                            .map(|generation| generation.loaded.model.identity.clone()),
                        previous_generation: previous.map(|generation| generation.generation),
                    }
                })
                .collect::<Vec<_>>();
            self.repository
                .activate(
                    identity,
                    &no_longer_active,
                    &persisted_slots,
                    generation,
                    now_epoch_ms,
                )
                .await?;
            let active = Arc::new(ModelGeneration { generation, loaded });
            for capability in &active.loaded.model.capabilities {
                let slot = slots.entry(capability.clone()).or_default();
                slot.previous = slot.active.replace(active.clone());
            }
            Ok(generation)
        }
        .await;
        match &result {
            Ok(generation) => base::log::info!(
                "Model activation completed: action=model_lifecycle, stage=activate, outcome=succeeded, model_id={}, version={}, revision={}, generation={}",
                identity.model_id,
                identity.version,
                identity.revision,
                generation
            ),
            Err(error) => {
                self.observability.observe_activation_failure();
                base::log::warn!(
                    "Model activation failed: action=model_lifecycle, stage=activate, outcome=failed, model_id={}, version={}, revision={}, error_code={}",
                    identity.model_id,
                    identity.version,
                    identity.revision,
                    error.code
                );
            }
        }
        result
    }

    pub async fn rollback(&self, capability: &str, now_epoch_ms: i64) -> ModelResult<u64> {
        self.rollback_with_context(
            capability,
            now_epoch_ms,
            self.runtime_context(HEALTH_TIMEOUT),
        )
        .await
    }

    pub async fn rollback_with_context(
        &self,
        capability: &str,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<u64> {
        let _lifecycle = self.lifecycle.lock().await;
        let result = self
            .rollback_locked(capability, now_epoch_ms, context)
            .await;
        match &result {
            Ok(generation) => base::log::info!(
                "Model rollback completed: action=model_lifecycle, stage=rollback, outcome=succeeded, capability={}, generation={}",
                capability,
                generation
            ),
            Err(error) => {
                self.observability.observe_activation_failure();
                base::log::warn!(
                    "Model rollback failed: action=model_lifecycle, stage=rollback, outcome=failed, capability={}, error_code={}",
                    capability,
                    error.code
                );
            }
        }
        result
    }

    pub async fn rollback_exact(
        &self,
        from: &ModelIdentity,
        to: &ModelIdentity,
        now_epoch_ms: i64,
    ) -> ModelResult<u64> {
        self.rollback_exact_with_context(
            from,
            to,
            now_epoch_ms,
            self.runtime_context(HEALTH_TIMEOUT),
        )
        .await
    }

    pub async fn rollback_exact_with_context(
        &self,
        from: &ModelIdentity,
        to: &ModelIdentity,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<u64> {
        let _lifecycle = self.lifecycle.lock().await;
        let result = async {
            if from == to {
                return Err(ModelError::new(
                    "model_rollback_conflict",
                    "rollback source and target must be different revisions",
                ));
            }
            let loaded = self.loaded.lock().await;
            let from_loaded = loaded.get(from).cloned().ok_or_else(|| {
                ModelError::new("model_not_active", "rollback source is not loaded")
            })?;
            let to_loaded = loaded.get(to).cloned().ok_or_else(|| {
                ModelError::new("model_runtime_unavailable", "rollback target is not loaded")
            })?;
            drop(loaded);
            let from_capabilities = from_loaded
                .model
                .capabilities
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let to_capabilities = to_loaded
                .model
                .capabilities
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            if from_capabilities != to_capabilities {
                return Err(ModelError::new(
                    "model_rollback_conflict",
                    "rollback revisions do not declare the same capability set",
                ));
            }
            let mut slots = self.slots.write().await;
            let active_from = slot_capabilities(&slots, from, SlotRelation::Active);
            let previous_from = slot_capabilities(&slots, from, SlotRelation::Previous);
            let active_to = slot_capabilities(&slots, to, SlotRelation::Active);
            let previous_to = slot_capabilities(&slots, to, SlotRelation::Previous);
            let committed = active_to == to_capabilities
                && previous_from == from_capabilities
                && active_from.is_empty()
                && previous_to.is_empty();
            if committed {
                let capability = from_capabilities.iter().next().ok_or_else(|| {
                    ModelError::new("model_rollback_conflict", "model has no capabilities")
                })?;
                return slots
                    .get(capability)
                    .and_then(|slot| slot.active.as_ref())
                    .map(|active| active.generation)
                    .ok_or_else(|| {
                        ModelError::new("model_rollback_conflict", "rollback slot disappeared")
                    });
            }
            let ready = active_from == from_capabilities
                && previous_to == to_capabilities
                && active_to.is_empty()
                && previous_from.is_empty();
            if !ready {
                return Err(ModelError::new(
                    "model_rollback_conflict",
                    "capability slots do not exactly match the requested rollback",
                ));
            }
            to_loaded
                .instance
                .health(context.with_local_maximum(HEALTH_TIMEOUT))
                .await?;
            let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
            let persisted = from_loaded
                .model
                .capabilities
                .iter()
                .map(|capability| PersistedCapabilitySlot {
                    capability: capability.clone(),
                    active_identity: to.clone(),
                    active_generation: generation,
                    previous_identity: Some(from.clone()),
                    previous_generation: slots
                        .get(capability)
                        .and_then(|slot| slot.active.as_ref())
                        .map(|active| active.generation),
                })
                .collect::<Vec<_>>();
            self.repository
                .activate(
                    to,
                    std::slice::from_ref(from),
                    &persisted,
                    generation,
                    now_epoch_ms,
                )
                .await?;
            let restored = Arc::new(ModelGeneration {
                generation,
                loaded: to_loaded,
            });
            for capability in &from_loaded.model.capabilities {
                let slot = slots.get_mut(capability).ok_or_else(|| {
                    ModelError::new("model_rollback_conflict", "rollback slot disappeared")
                })?;
                let replaced = slot.active.replace(restored.clone());
                slot.previous = replaced;
            }
            Ok(generation)
        }
        .await;
        match &result {
            Ok(generation) => base::log::info!(
                "Model rollback completed: action=model_lifecycle, stage=rollback, outcome=succeeded, model_id={}, version={}, revision={}, generation={}",
                to.model_id,
                to.version,
                to.revision,
                generation
            ),
            Err(error) => {
                self.observability.observe_activation_failure();
                base::log::warn!(
                    "Model rollback failed: action=model_lifecycle, stage=rollback, outcome=failed, model_id={}, version={}, revision={}, error_code={}",
                    to.model_id,
                    to.version,
                    to.revision,
                    error.code
                );
            }
        }
        result
    }

    async fn rollback_locked(
        &self,
        capability: &str,
        now_epoch_ms: i64,
        context: RuntimeCallContext,
    ) -> ModelResult<u64> {
        let mut slots = self.slots.write().await;
        let slot = slots
            .get(capability)
            .ok_or_else(|| ModelError::new("model_not_active", "capability has no active model"))?;
        let previous = slot.previous.clone().ok_or_else(|| {
            ModelError::new(
                "model_rollback_unavailable",
                "capability has no previous model",
            )
        })?;
        let current = slot.active.clone();
        previous
            .loaded
            .instance
            .health(context.with_local_maximum(HEALTH_TIMEOUT))
            .await?;
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
        let replaced = current
            .as_ref()
            .filter(|active| {
                !slots.iter().any(|(other_capability, other_slot)| {
                    other_capability != capability
                        && other_slot.active.as_ref().is_some_and(|candidate| {
                            candidate.loaded.model.identity == active.loaded.model.identity
                        })
                })
            })
            .map(|active| active.loaded.model.identity.clone())
            .into_iter()
            .collect::<Vec<_>>();
        let persisted_slot = PersistedCapabilitySlot {
            capability: capability.to_string(),
            active_identity: previous.loaded.model.identity.clone(),
            active_generation: generation,
            previous_identity: current
                .as_ref()
                .map(|generation| generation.loaded.model.identity.clone()),
            previous_generation: current.as_ref().map(|generation| generation.generation),
        };
        self.repository
            .activate(
                &previous.loaded.model.identity,
                &replaced,
                &[persisted_slot],
                generation,
                now_epoch_ms,
            )
            .await?;
        let restored = Arc::new(ModelGeneration {
            generation,
            loaded: previous.loaded.clone(),
        });
        let slot = slots
            .get_mut(capability)
            .ok_or_else(|| ModelError::new("model_not_active", "capability slot disappeared"))?;
        let replaced = slot.active.replace(restored);
        slot.previous = replaced;
        Ok(generation)
    }

    pub async fn retire_previous(
        &self,
        capability: &str,
        _now_epoch_ms: i64,
    ) -> ModelResult<Option<ModelIdentity>> {
        let _lifecycle = self.lifecycle.lock().await;
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(capability) else {
            return Ok(None);
        };
        let Some(previous) = slot.previous.as_ref() else {
            return Ok(None);
        };
        if previous.loaded.in_flight.load(Ordering::Acquire) != 0 {
            return Err(ModelError::new(
                "model_in_use",
                "previous model still has in-flight tasks",
            ));
        }
        let identity = previous.loaded.model.identity.clone();
        self.repository.clear_previous(capability).await?;
        slot.previous = None;
        Ok(Some(identity))
    }
}

#[derive(Clone, Copy)]
enum SlotRelation {
    Active,
    Previous,
}

fn slot_capabilities(
    slots: &HashMap<String, CapabilitySlot>,
    identity: &ModelIdentity,
    relation: SlotRelation,
) -> HashSet<String> {
    slots
        .iter()
        .filter_map(|(capability, slot)| {
            let generation = match relation {
                SlotRelation::Active => slot.active.as_ref(),
                SlotRelation::Previous => slot.previous.as_ref(),
            };
            generation
                .filter(|generation| generation.loaded.model.identity == *identity)
                .map(|_| capability.clone())
        })
        .collect()
}
