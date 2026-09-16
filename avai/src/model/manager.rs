use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use base::{
    tokio::sync::{Mutex, RwLock},
    tokio_util::sync::CancellationToken,
};

use super::{
    InferenceResult, InstalledModel, ModelError, ModelIdentity, ModelInstance, ModelRepository,
    ModelResult, ModelState, ResultSchema, RuntimeCallContext, RuntimeInput, RuntimeProvider,
    package::load_installed_execution_contract,
    repository::{CapabilityRecovery, PersistedCapabilitySlot},
};

#[derive(Debug, Clone, Copy)]
pub struct ModelManagerConfig {
    pub max_loaded_models: usize,
    pub max_memory_mb: u64,
    pub max_vram_mb: u64,
}

impl Default for ModelManagerConfig {
    fn default() -> Self {
        Self {
            max_loaded_models: 8,
            max_memory_mb: 16 * 1024,
            max_vram_mb: 16 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub identity: ModelIdentity,
    pub runtime: String,
    pub generation: Option<u64>,
    pub active_capabilities: Vec<String>,
    pub in_flight_tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelObservation {
    pub loaded: bool,
    pub runtime_available: bool,
    pub active_capabilities: Vec<String>,
    pub previous_capabilities: Vec<String>,
    pub generation: Option<u64>,
    pub in_flight_tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthReconcile {
    Healthy,
    RolledBack {
        failed: ModelIdentity,
        restored: Vec<RecoveredCapability>,
        cleared_capabilities: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredCapability {
    pub capability: String,
    pub identity: ModelIdentity,
    pub generation: u64,
}

#[derive(Clone)]
pub struct ModelManager {
    repository: ModelRepository,
    providers: Arc<HashMap<String, Arc<dyn RuntimeProvider>>>,
    config: ModelManagerConfig,
    loaded: Arc<Mutex<HashMap<ModelIdentity, Arc<LoadedModel>>>>,
    slots: Arc<RwLock<HashMap<String, CapabilitySlot>>>,
    next_generation: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<()>>,
    runtime_cancellation: CancellationToken,
}

struct LoadedModel {
    model: InstalledModel,
    instance: Arc<dyn ModelInstance>,
    result_schema: ResultSchema,
    in_flight: AtomicUsize,
}

struct ModelGeneration {
    generation: u64,
    loaded: Arc<LoadedModel>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadFailureEvidence {
    IntrinsicModel,
    RuntimeAvailability,
}

#[derive(Default)]
struct CapabilitySlot {
    active: Option<Arc<ModelGeneration>>,
    previous: Option<Arc<ModelGeneration>>,
}

pub struct ActiveModel {
    generation: Arc<ModelGeneration>,
}

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

const PRELOAD_TIMEOUT: Duration = Duration::from_secs(30);
const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub async fn open(
        repository: ModelRepository,
        providers: Vec<Arc<dyn RuntimeProvider>>,
        config: ModelManagerConfig,
    ) -> ModelResult<Self> {
        Self::open_with_cancellation(repository, providers, config, CancellationToken::new()).await
    }

    pub async fn open_with_cancellation(
        repository: ModelRepository,
        providers: Vec<Arc<dyn RuntimeProvider>>,
        config: ModelManagerConfig,
        runtime_cancellation: CancellationToken,
    ) -> ModelResult<Self> {
        if config.max_loaded_models == 0 || config.max_memory_mb == 0 {
            return Err(ModelError::new(
                "invalid_model_manager_config",
                "loaded model count and memory budget must be positive",
            ));
        }
        let mut by_runtime = HashMap::new();
        for provider in providers {
            let runtime = provider.descriptor().runtime;
            if by_runtime.insert(runtime.clone(), provider).is_some() {
                return Err(ModelError::new(
                    "duplicate_runtime_provider",
                    format!("runtime provider is registered twice: {runtime}"),
                ));
            }
        }
        let next_generation = repository.max_generation().await?.saturating_add(1);
        let manager = Self {
            repository,
            providers: Arc::new(by_runtime),
            config,
            loaded: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(RwLock::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(next_generation)),
            lifecycle: Arc::new(Mutex::new(())),
            runtime_cancellation,
        };
        manager.restore_capability_slots().await?;
        Ok(manager)
    }

    async fn restore_capability_slots(&self) -> ModelResult<()> {
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
        let instance = provider
            .preload(&model, self.runtime_context(PRELOAD_TIMEOUT))
            .await?;
        if let Err(error) = validate_instance(&model, &instance) {
            self.cleanup_rejected_instance(&instance).await;
            return Err(error);
        }
        if let Err(error) = instance
            .self_test(&model.self_tests, self.runtime_context(SELF_TEST_TIMEOUT))
            .await
        {
            self.cleanup_rejected_instance(&instance).await;
            return Err(error);
        }
        if let Err(error) = instance.health(self.runtime_context(HEALTH_TIMEOUT)).await {
            self.cleanup_rejected_instance(&instance).await;
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
        Ok(Arc::new(ModelGeneration { generation, loaded }))
    }

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
        let instance = match provider
            .preload(&model, context.with_local_maximum(PRELOAD_TIMEOUT))
            .await
        {
            Ok(instance) => instance,
            Err(error) => {
                self.record_load_failure(identity, &error, now_epoch_ms)
                    .await?;
                return Err(error);
            }
        };
        if let Err(error) = validate_instance(&model, &instance) {
            self.cleanup_rejected_instance(&instance).await;
            self.record_load_failure(identity, &error, now_epoch_ms)
                .await?;
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
            self.record_load_failure(identity, &error, now_epoch_ms)
                .await?;
            return Err(error);
        }
        if let Err(error) = instance
            .health(context.with_local_maximum(HEALTH_TIMEOUT))
            .await
        {
            self.cleanup_rejected_instance(&instance).await;
            self.record_load_failure(identity, &error, now_epoch_ms)
                .await?;
            return Err(error);
        }
        self.repository
            .set_state(identity, ModelState::Ready, None, now_epoch_ms)
            .await?;
        self.loaded.lock().await.insert(
            identity.clone(),
            Arc::new(LoadedModel {
                model,
                instance,
                result_schema: execution_contract.result_schema,
                in_flight: AtomicUsize::new(0),
            }),
        );
        Ok(())
    }

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
        let loaded = self
            .loaded
            .lock()
            .await
            .get(identity)
            .cloned()
            .ok_or_else(|| ModelError::new("model_not_ready", "model must be preloaded first"))?;
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
        self.rollback_locked(capability, now_epoch_ms, context)
            .await
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
        if from == to {
            return Err(ModelError::new(
                "model_rollback_conflict",
                "rollback source and target must be different revisions",
            ));
        }
        let loaded = self.loaded.lock().await;
        let from_loaded = loaded
            .get(from)
            .cloned()
            .ok_or_else(|| ModelError::new("model_not_active", "rollback source is not loaded"))?;
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
            let instance = match provider
                .preload(&model, self.runtime_context(PRELOAD_TIMEOUT))
                .await
            {
                Ok(instance) => instance,
                Err(error) => {
                    self.record_load_failure(identity, &error, now_epoch_ms)
                        .await?;
                    return Err(error);
                }
            };
            if let Err(error) = validate_instance(&model, &instance) {
                self.cleanup_rejected_instance(&instance).await;
                self.record_load_failure(identity, &error, now_epoch_ms)
                    .await?;
                return Err(error);
            }
            if let Err(error) = instance
                .self_test(&model.self_tests, self.runtime_context(SELF_TEST_TIMEOUT))
                .await
            {
                self.cleanup_rejected_instance(&instance).await;
                self.record_load_failure(identity, &error, now_epoch_ms)
                    .await?;
                return Err(error);
            }
            if let Err(error) = instance.health(self.runtime_context(HEALTH_TIMEOUT)).await {
                self.cleanup_rejected_instance(&instance).await;
                self.record_load_failure(identity, &error, now_epoch_ms)
                    .await?;
                return Err(error);
            }
            if model.state == ModelState::Installed {
                self.repository
                    .set_state(identity, ModelState::Ready, None, now_epoch_ms)
                    .await?;
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
        Ok(())
    }

    async fn cleanup_rejected_instance(&self, instance: &Arc<dyn ModelInstance>) {
        if let Err(error) = instance.unload(self.runtime_context(UNLOAD_TIMEOUT)).await {
            base::log::warn!(
                "model instance cleanup failed: action=unload_rejected_instance, error_code={}",
                error.code
            );
        }
    }

    async fn record_load_failure(
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
        restored.sort_by(|left, right| left.capability.cmp(&right.capability));
        cleared_capabilities.sort();
        Ok(HealthReconcile::RolledBack {
            failed,
            restored,
            cleared_capabilities,
        })
    }

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
        let mut generation = None;
        for (capability, slot) in slots.iter() {
            if let Some(active) = &slot.active
                && active.loaded.model.identity == *identity
            {
                active_capabilities.push(capability.clone());
                generation = Some(active.generation);
            }
            if slot
                .previous
                .as_ref()
                .is_some_and(|previous| previous.loaded.model.identity == *identity)
            {
                previous_capabilities.push(capability.clone());
            }
        }
        active_capabilities.sort();
        previous_capabilities.sort();
        ModelObservation {
            loaded: candidate.is_some(),
            runtime_available: candidate.map_or_else(
                || false,
                |loaded| self.providers.contains_key(&loaded.model.runtime),
            ),
            active_capabilities,
            previous_capabilities,
            generation,
            in_flight_tasks: candidate
                .map(|loaded| loaded.in_flight.load(Ordering::Acquire))
                .unwrap_or_default(),
        }
    }

    pub fn runtime_available(&self, runtime: &str) -> bool {
        self.providers.contains_key(runtime)
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

    fn runtime_context(&self, maximum: Duration) -> RuntimeCallContext {
        RuntimeCallContext::local(maximum, self.runtime_cancellation.clone())
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

    async fn ensure_budget(&self, model: &InstalledModel) -> ModelResult<()> {
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

fn classify_load_failure(error: &ModelError) -> LoadFailureEvidence {
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

fn validate_instance(model: &InstalledModel, instance: &Arc<dyn ModelInstance>) -> ModelResult<()> {
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

#[cfg(test)]
mod tests {
    use super::{LoadFailureEvidence, ModelError, classify_load_failure};

    #[test]
    fn load_failure_classification_separates_runtime_availability_from_model_evidence() {
        for code in [
            "model_runtime_provider_unavailable",
            "model_runtime_stale_handle",
            "model_runtime_busy",
            "model_runtime_deadline_exceeded",
            "model_runtime_cancelled",
            "model_runtime_protocol_mismatch",
            "model_runtime_protocol_violation",
            "model_runtime_response_invalid",
        ] {
            assert!(matches!(
                classify_load_failure(&ModelError::new(code, "test")),
                LoadFailureEvidence::RuntimeAvailability
            ));
        }
        for code in [
            "model_runtime_contract_mismatch",
            "model_preload_failed",
            "model_self_test_failed",
            "model_health_failed",
        ] {
            assert!(matches!(
                classify_load_failure(&ModelError::new(code, "test")),
                LoadFailureEvidence::IntrinsicModel
            ));
        }
    }
}
