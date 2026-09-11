use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use base::tokio::sync::{Mutex, RwLock};

use super::{
    InferenceResult, InstalledModel, ModelError, ModelIdentity, ModelInstance, ModelRepository,
    ModelResult, ModelState, RuntimeProvider, repository::PersistedCapabilitySlot,
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
pub enum HealthReconcile {
    Healthy,
    RolledBack {
        failed: ModelIdentity,
        restored: ModelIdentity,
        generation: u64,
    },
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
}

struct LoadedModel {
    model: InstalledModel,
    instance: Arc<dyn ModelInstance>,
    in_flight: AtomicUsize,
}

struct ModelGeneration {
    generation: u64,
    loaded: Arc<LoadedModel>,
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

    pub async fn infer(&self, input: Vec<u8>) -> ModelResult<InferenceResult> {
        self.generation.loaded.instance.infer(input).await
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
    pub async fn open(
        repository: ModelRepository,
        providers: Vec<Arc<dyn RuntimeProvider>>,
        config: ModelManagerConfig,
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
        for slot in persisted {
            let active = self
                .restore_generation(&slot.active_identity, slot.active_generation)
                .await?;
            if active.loaded.model.state != ModelState::Active {
                return Err(ModelError::new(
                    "model_slot_invalid",
                    "persisted active slot references a model that is not active",
                ));
            }
            if !active.loaded.model.capabilities.contains(&slot.capability) {
                return Err(ModelError::new(
                    "model_slot_invalid",
                    "active model does not provide persisted capability",
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
                slot.capability,
                CapabilitySlot {
                    active: Some(active),
                    previous,
                },
            );
        }
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
        self.ensure_budget(&model).await?;
        let instance = provider.preload(&model).await?;
        validate_instance(&model, &instance)?;
        instance.self_test(&model.self_tests).await?;
        instance.health().await?;
        let loaded = Arc::new(LoadedModel {
            model,
            instance,
            in_flight: AtomicUsize::new(0),
        });
        self.loaded
            .lock()
            .await
            .insert(identity.clone(), loaded.clone());
        Ok(Arc::new(ModelGeneration { generation, loaded }))
    }

    pub async fn preload(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<()> {
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
        self.ensure_budget(&model).await?;
        let instance = match provider.preload(&model).await {
            Ok(instance) => instance,
            Err(error) => {
                self.repository
                    .set_state(identity, ModelState::Failed, None, now_epoch_ms)
                    .await?;
                return Err(error);
            }
        };
        if let Err(error) = validate_instance(&model, &instance) {
            self.repository
                .set_state(identity, ModelState::Failed, None, now_epoch_ms)
                .await?;
            return Err(error);
        }
        if let Err(error) = instance.self_test(&model.self_tests).await {
            self.repository
                .set_state(identity, ModelState::Failed, None, now_epoch_ms)
                .await?;
            return Err(error);
        }
        if let Err(error) = instance.health().await {
            self.repository
                .set_state(identity, ModelState::Failed, None, now_epoch_ms)
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
                in_flight: AtomicUsize::new(0),
            }),
        );
        Ok(())
    }

    pub async fn activate(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<u64> {
        let _lifecycle = self.lifecycle.lock().await;
        let loaded = self
            .loaded
            .lock()
            .await
            .get(identity)
            .cloned()
            .ok_or_else(|| ModelError::new("model_not_ready", "model must be preloaded first"))?;
        let slots = self.slots.write().await;
        let already_active = loaded.model.capabilities.iter().all(|capability| {
            slots
                .get(capability)
                .and_then(|slot| slot.active.as_ref())
                .is_some_and(|active| active.loaded.model.identity == *identity)
        });
        if already_active
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
        loaded.instance.health().await?;
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
                &[],
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
        let _lifecycle = self.lifecycle.lock().await;
        self.rollback_locked(capability, now_epoch_ms, false).await
    }

    async fn rollback_locked(
        &self,
        capability: &str,
        now_epoch_ms: i64,
        failed_current: bool,
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
        previous.loaded.instance.health().await?;
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
            previous_identity: (!failed_current)
                .then_some(current.as_ref())
                .flatten()
                .map(|generation| generation.loaded.model.identity.clone()),
            previous_generation: (!failed_current)
                .then_some(current.as_ref())
                .flatten()
                .map(|generation| generation.generation),
        };
        let failed_models = if failed_current {
            current
                .as_ref()
                .map(|generation| generation.loaded.model.identity.clone())
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.repository
            .activate(
                &previous.loaded.model.identity,
                &replaced,
                &failed_models,
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
        slot.previous = (!failed_current).then_some(replaced).flatten();
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

    pub async fn unload(&self, identity: &ModelIdentity, now_epoch_ms: i64) -> ModelResult<()> {
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
        let mut loaded = self.loaded.lock().await;
        let Some(candidate) = loaded.get(identity) else {
            return Ok(());
        };
        if candidate.in_flight.load(Ordering::Acquire) != 0 {
            return Err(ModelError::new(
                "model_in_use",
                "model still has in-flight tasks",
            ));
        }
        self.repository
            .set_state(identity, ModelState::Installed, None, now_epoch_ms)
            .await?;
        loaded.remove(identity);
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
        if active.loaded.instance.health().await.is_ok() {
            return Ok(HealthReconcile::Healthy);
        }
        let failed = active.loaded.model.identity.clone();
        let generation = self.rollback_locked(capability, now_epoch_ms, true).await?;
        let restored = self
            .slots
            .read()
            .await
            .get(capability)
            .and_then(|slot| slot.active.as_ref())
            .map(|active| active.loaded.model.identity.clone())
            .ok_or_else(|| ModelError::new("model_not_active", "rollback did not restore model"))?;
        Ok(HealthReconcile::RolledBack {
            failed,
            restored,
            generation,
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
