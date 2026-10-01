mod activation;
mod capture;
mod lifecycle;
mod observation;
mod recovery;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use base::{
    tokio::sync::{Mutex, RwLock},
    tokio_util::sync::CancellationToken,
};

use crate::observability::Observability;

use super::{
    InferenceResult, InstalledModel, ModelError, ModelIdentity, ModelInstance, ModelRepository,
    ModelResult, ModelState, ResultSchema, RuntimeCallContext, RuntimeInput, RuntimeProvider,
    RuntimeVariant,
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
    pub active_bindings: Vec<CapabilityGeneration>,
    pub previous_bindings: Vec<CapabilityGeneration>,
    pub generation: Option<u64>,
    pub in_flight_tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGeneration {
    pub capability: String,
    pub generation: u64,
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
    observability: Arc<Observability>,
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

const PRELOAD_TIMEOUT: Duration = Duration::from_secs(30);
const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(10);

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
        Self::open_with_observability(
            repository,
            providers,
            config,
            runtime_cancellation,
            Arc::new(Observability::new()),
        )
        .await
    }

    pub async fn open_with_observability(
        repository: ModelRepository,
        providers: Vec<Arc<dyn RuntimeProvider>>,
        config: ModelManagerConfig,
        runtime_cancellation: CancellationToken,
        observability: Arc<Observability>,
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
            observability,
        };
        manager.restore_capability_slots().await?;
        manager.refresh_ready_models().await;
        Ok(manager)
    }
}

#[cfg(test)]
use lifecycle::classify_load_failure;

#[cfg(test)]
#[path = "../../../tests/unit/model/manager.rs"]
mod tests;
