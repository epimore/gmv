use super::{
    Arc, AtomicUsize, CapabilityRecovery, CapabilitySlot, HEALTH_TIMEOUT, HashMap, HashSet,
    HealthReconcile, Instant, LoadedModel, ModelError, ModelGeneration, ModelIdentity,
    ModelManager, ModelResult, ModelState, Ordering, PRELOAD_TIMEOUT, PersistedCapabilitySlot,
    RecoveredCapability, SELF_TEST_TIMEOUT, load_installed_execution_contract,
};

use super::lifecycle::validate_instance;

impl ModelManager {
    pub(super) async fn restore_capability_slots(&self) -> ModelResult<()> {
        self.repository.reconcile_unreferenced_ready().await?;
        let persisted = self.repository.list_slots().await?;
        let models = self.repository.list().await?;
        let active_identities = persisted
            .iter()
            .map(|slot| slot.active_identity.clone())
            .collect::<HashSet<_>>();
        if models.iter().any(|model| {
            model.state == ModelState::Active && !active_identities.contains(&model.identity)
        }) {
            return Err(ModelError::new(
                "model_slot_invalid",
                "active model has no persisted capability slot",
            ));
        }
        let mut restored_identities = HashSet::new();
        for first in &persisted {
            if !restored_identities.insert(first.active_identity.clone()) {
                continue;
            }
            let model_slots = persisted
                .iter()
                .filter(|slot| slot.active_identity == first.active_identity)
                .collect::<Vec<_>>();
            match self
                .restore_generation(&first.active_identity, first.active_generation)
                .await
            {
                Ok(_) => self.restore_healthy_slots(&model_slots).await?,
                Err(active_error) => {
                    self.restore_failed_active(&first.active_identity, &model_slots, &active_error)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn restore_healthy_slots(
        &self,
        persisted: &[&PersistedCapabilitySlot],
    ) -> ModelResult<()> {
        for slot in persisted {
            let active = self
                .restore_generation(&slot.active_identity, slot.active_generation)
                .await?;
            if active.loaded.model.state != ModelState::Active
                || !active.loaded.model.capabilities.contains(&slot.capability)
            {
                return Err(ModelError::new(
                    "model_slot_invalid",
                    "persisted active slot does not match its model",
                ));
            }
            let previous = match (&slot.previous_identity, slot.previous_generation) {
                (Some(identity), Some(generation)) => {
                    let previous = self.restore_generation(identity, generation).await?;
                    if !previous
                        .loaded
                        .model
                        .capabilities
                        .contains(&slot.capability)
                    {
                        return Err(ModelError::new(
                            "model_slot_invalid",
                            "previous model does not provide persisted capability",
                        ));
                    }
                    Some(previous)
                }
                (None, None) => None,
                _ => {
                    return Err(ModelError::new(
                        "model_slot_invalid",
                        "previous model slot is incomplete",
                    ));
                }
            };
            self.slots.write().await.insert(
                slot.capability.clone(),
                CapabilitySlot {
                    active: Some(active),
                    previous,
                },
            );
        }
        Ok(())
    }

    async fn restore_failed_active(
        &self,
        failed: &ModelIdentity,
        persisted: &[&PersistedCapabilitySlot],
        active_error: &ModelError,
    ) -> ModelResult<()> {
        let mut recoveries = Vec::with_capacity(persisted.len());
        let mut restored = Vec::with_capacity(persisted.len());
        for slot in persisted {
            let (previous_identity, previous_generation) = slot
                .previous_identity
                .as_ref()
                .zip(slot.previous_generation)
                .ok_or_else(|| startup_recovery_error(failed, active_error, None))?;
            let previous = self
                .restore_generation(previous_identity, previous_generation)
                .await
                .map_err(|error| startup_recovery_error(failed, active_error, Some(&error)))?;
            if !previous
                .loaded
                .model
                .capabilities
                .contains(&slot.capability)
            {
                return Err(startup_recovery_error(failed, active_error, None));
            }
            let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
            recoveries.push(CapabilityRecovery {
                capability: slot.capability.clone(),
                expected_active_generation: slot.active_generation,
                replacement: Some((previous_identity.clone(), generation)),
            });
            restored.push((slot.capability.clone(), previous, generation));
        }
        self.repository
            .recover_failed_active(
                failed,
                &recoveries,
                current_epoch_ms()?,
                &format!("active restore failed: {}", active_error.code),
            )
            .await?;
        let mut slots = self.slots.write().await;
        for (capability, previous, generation) in restored {
            slots.insert(
                capability,
                CapabilitySlot {
                    active: Some(Arc::new(ModelGeneration {
                        generation,
                        loaded: previous.loaded.clone(),
                    })),
                    previous: None,
                },
            );
        }
        clear_failed_previous_references(&mut slots, failed);
        Ok(())
    }

    async fn restore_generation(
        &self,
        identity: &ModelIdentity,
        generation: u64,
    ) -> ModelResult<Arc<ModelGeneration>> {
        if let Some(loaded) = self.loaded.lock().await.get(identity).cloned() {
            return Ok(Arc::new(ModelGeneration { generation, loaded }));
        }
        let model = self.repository.get(identity).await?.ok_or_else(|| {
            ModelError::new("model_slot_invalid", "persisted slot model does not exist")
        })?;
        if !matches!(model.state, ModelState::Active | ModelState::Ready) {
            return Err(ModelError::new(
                "model_slot_invalid",
                "persisted slot references a model that is not active or ready",
            ));
        }
        let provider = self.providers.get(&model.runtime).ok_or_else(|| {
            ModelError::new(
                "model_runtime_unavailable",
                format!("runtime provider is unavailable: {}", model.runtime),
            )
        })?;
        let execution_contract = load_installed_execution_contract(&model)?;
        self.ensure_budget(&model).await?;
        let preload_started = Instant::now();
        let instance = match provider
            .preload(&model, self.runtime_context(PRELOAD_TIMEOUT))
            .await
        {
            Ok(instance) => instance,
            Err(error) => {
                self.observability
                    .observe_preload(preload_started.elapsed());
                base::log::warn!(
                    "Model startup restore failed: action=model_lifecycle, stage=startup_restore, outcome=failed, runtime={}, error_code={}",
                    model.runtime,
                    error.code
                );
                return Err(error);
            }
        };
        if let Err(error) = validate_instance(&model, &instance) {
            self.cleanup_rejected_instance(&instance).await;
            self.observability
                .observe_preload(preload_started.elapsed());
            return Err(error);
        }
        if let Err(error) = instance
            .self_test(&model.self_tests, self.runtime_context(SELF_TEST_TIMEOUT))
            .await
        {
            self.cleanup_rejected_instance(&instance).await;
            self.observability.observe_self_test_failure();
            self.observability
                .observe_preload(preload_started.elapsed());
            base::log::warn!(
                "Model startup self-test failed: action=model_lifecycle, stage=self_test, outcome=failed, runtime={}, error_code={}",
                model.runtime,
                error.code
            );
            return Err(error);
        }
        if let Err(error) = instance.health(self.runtime_context(HEALTH_TIMEOUT)).await {
            self.cleanup_rejected_instance(&instance).await;
            self.observability
                .observe_preload(preload_started.elapsed());
            return Err(error);
        }
        let loaded = Arc::new(LoadedModel {
            model,
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
        base::log::info!(
            "Model startup restore succeeded: action=model_lifecycle, stage=startup_restore, outcome=succeeded, model_id={}, version={}, revision={}, runtime={}",
            identity.model_id,
            identity.version,
            identity.revision,
            loaded.model.runtime
        );
        Ok(Arc::new(ModelGeneration { generation, loaded }))
    }

    pub async fn reconcile_active_health(
        &self,
        capability: &str,
        now_epoch_ms: i64,
    ) -> ModelResult<HealthReconcile> {
        let _lifecycle = self.lifecycle.lock().await;
        let active = self
            .slots
            .read()
            .await
            .get(capability)
            .and_then(|slot| slot.active.clone())
            .ok_or_else(|| ModelError::new("model_not_active", "capability has no active model"))?;
        if active
            .loaded
            .instance
            .health(self.runtime_context(HEALTH_TIMEOUT))
            .await
            .is_ok()
        {
            return Ok(HealthReconcile::Healthy);
        }
        let failed = active.loaded.model.identity.clone();
        let failed_slots = self
            .slots
            .read()
            .await
            .iter()
            .filter_map(|(capability, slot)| {
                slot.active
                    .as_ref()
                    .filter(|active| active.loaded.model.identity == failed)
                    .map(|active| (capability.clone(), active.clone(), slot.previous.clone()))
            })
            .collect::<Vec<_>>();
        let mut recoveries = Vec::with_capacity(failed_slots.len());
        let mut restored = Vec::new();
        let mut restored_generations = Vec::new();
        let mut cleared_capabilities = Vec::new();
        for (capability, failed_generation, previous) in &failed_slots {
            let replacement = if let Some(previous) = previous
                && previous.loaded.model.identity != failed
                && previous
                    .loaded
                    .instance
                    .health(self.runtime_context(HEALTH_TIMEOUT))
                    .await
                    .is_ok()
            {
                let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
                restored.push(RecoveredCapability {
                    capability: capability.clone(),
                    identity: previous.loaded.model.identity.clone(),
                    generation,
                });
                restored_generations.push((
                    capability.clone(),
                    previous.loaded.clone(),
                    generation,
                ));
                Some((previous.loaded.model.identity.clone(), generation))
            } else {
                cleared_capabilities.push(capability.clone());
                None
            };
            recoveries.push(CapabilityRecovery {
                capability: capability.clone(),
                expected_active_generation: failed_generation.generation,
                replacement,
            });
        }
        self.repository
            .recover_failed_active(
                &failed,
                &recoveries,
                now_epoch_ms,
                "active health check failed",
            )
            .await?;
        let mut slots = self.slots.write().await;
        for recovery in &recoveries {
            slots.remove(&recovery.capability);
        }
        for (capability, loaded, generation) in restored_generations {
            slots.insert(
                capability,
                CapabilitySlot {
                    active: Some(Arc::new(ModelGeneration { generation, loaded })),
                    previous: None,
                },
            );
        }
        clear_failed_previous_references(&mut slots, &failed);
        drop(slots);
        self.refresh_ready_models().await;
        restored.sort_by(|left, right| left.capability.cmp(&right.capability));
        cleared_capabilities.sort();
        base::log::warn!(
            "Model health fallback completed: action=model_lifecycle, stage=health, outcome=fallback, model_id={}, version={}, revision={}",
            failed.model_id,
            failed.version,
            failed.revision
        );
        Ok(HealthReconcile::RolledBack {
            failed,
            restored,
            cleared_capabilities,
        })
    }
}

fn clear_failed_previous_references(
    slots: &mut HashMap<String, CapabilitySlot>,
    failed: &ModelIdentity,
) {
    for slot in slots.values_mut() {
        if slot
            .previous
            .as_ref()
            .is_some_and(|previous| previous.loaded.model.identity == *failed)
        {
            slot.previous = None;
        }
    }
}

fn startup_recovery_error(
    failed: &ModelIdentity,
    active_error: &ModelError,
    previous_error: Option<&ModelError>,
) -> ModelError {
    let previous = previous_error
        .map(|error| format!("; previous recovery failed: {}", error.code))
        .unwrap_or_else(|| "; healthy previous model is unavailable".to_string());
    ModelError::new(
        "model_startup_recovery_failed",
        format!(
            "cannot recover active model {}/{}/{}: {}{previous}",
            failed.model_id, failed.version, failed.revision, active_error.code
        ),
    )
}

fn current_epoch_ms() -> ModelResult<i64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| ModelError::io("read current time", error))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| ModelError::new("model_time_invalid", "current time is too large"))
}
