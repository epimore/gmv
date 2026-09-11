use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use base::tokio::sync::Notify;

use super::{InstalledModel, ModelError, ModelIdentity, ModelResult, SelfTestCase};

pub type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = ModelResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDescriptor {
    pub runtime: String,
    pub version: String,
}

pub trait RuntimeProvider: Send + Sync {
    fn descriptor(&self) -> RuntimeDescriptor;

    fn preload<'a>(
        &'a self,
        model: &'a InstalledModel,
    ) -> RuntimeFuture<'a, Arc<dyn ModelInstance>>;
}

pub trait ModelInstance: Send + Sync {
    fn identity(&self) -> &ModelIdentity;
    fn runtime(&self) -> &str;
    fn capabilities(&self) -> &[String];

    fn self_test<'a>(&'a self, cases: &'a [SelfTestCase]) -> RuntimeFuture<'a, ()>;

    fn health<'a>(&'a self) -> RuntimeFuture<'a, ()>;

    fn infer<'a>(&'a self, input: Vec<u8>) -> RuntimeFuture<'a, InferenceResult>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceResult {
    pub output: Vec<u8>,
    pub actual_model: gmv_protocol::avai::v1::ModelRef,
}

#[derive(Debug, Clone, Default)]
pub struct FakeRuntimeBehavior {
    pub fail_preload: bool,
    pub fail_preload_model: Option<String>,
    pub fail_self_test: bool,
    pub fail_self_test_model: Option<String>,
    pub fail_health: bool,
    pub block_inference: bool,
}

#[derive(Clone)]
pub struct FakeRuntimeProvider {
    descriptor: RuntimeDescriptor,
    behavior: FakeRuntimeBehavior,
    started: Arc<AtomicUsize>,
    release: Arc<Notify>,
    unhealthy_models: Arc<RwLock<HashSet<String>>>,
    pause_health: Arc<AtomicBool>,
    health_started: Arc<AtomicUsize>,
    health_release: Arc<Notify>,
    dropped: Arc<Mutex<HashMap<String, usize>>>,
}

impl FakeRuntimeProvider {
    pub fn new(runtime: impl Into<String>, behavior: FakeRuntimeBehavior) -> Self {
        Self {
            descriptor: RuntimeDescriptor {
                runtime: runtime.into(),
                version: "fake-v1".to_string(),
            },
            behavior,
            started: Arc::new(AtomicUsize::new(0)),
            release: Arc::new(Notify::new()),
            unhealthy_models: Arc::new(RwLock::new(HashSet::new())),
            pause_health: Arc::new(AtomicBool::new(false)),
            health_started: Arc::new(AtomicUsize::new(0)),
            health_release: Arc::new(Notify::new()),
            dropped: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn started_inferences(&self) -> usize {
        self.started.load(Ordering::Acquire)
    }

    pub fn release_inferences(&self) {
        self.release.notify_waiters();
    }

    pub fn set_model_unhealthy(&self, model_id: &str, unhealthy: bool) {
        let mut models = self
            .unhealthy_models
            .write()
            .expect("health state poisoned");
        if unhealthy {
            models.insert(model_id.to_string());
        } else {
            models.remove(model_id);
        }
    }

    pub fn pause_health_checks(&self) {
        self.pause_health.store(true, Ordering::Release);
    }

    pub fn started_health_checks(&self) -> usize {
        self.health_started.load(Ordering::Acquire)
    }

    pub fn release_health_checks(&self) {
        self.pause_health.store(false, Ordering::Release);
        self.health_release.notify_waiters();
    }

    pub fn dropped_instances(&self, model_id: &str) -> usize {
        self.dropped
            .lock()
            .expect("drop state poisoned")
            .get(model_id)
            .copied()
            .unwrap_or_default()
    }
}

impl RuntimeProvider for FakeRuntimeProvider {
    fn descriptor(&self) -> RuntimeDescriptor {
        self.descriptor.clone()
    }

    fn preload<'a>(
        &'a self,
        model: &'a InstalledModel,
    ) -> RuntimeFuture<'a, Arc<dyn ModelInstance>> {
        Box::pin(async move {
            if self.behavior.fail_preload
                || self
                    .behavior
                    .fail_preload_model
                    .as_ref()
                    .is_some_and(|model_id| model_id == &model.identity.model_id)
            {
                return Err(ModelError::new(
                    "model_preload_failed",
                    "fake runtime preload failure",
                ));
            }
            Ok(Arc::new(FakeModelInstance {
                identity: model.identity.clone(),
                runtime: model.runtime.clone(),
                capabilities: model.capabilities.clone(),
                behavior: self.behavior.clone(),
                started: self.started.clone(),
                release: self.release.clone(),
                unhealthy_models: self.unhealthy_models.clone(),
                pause_health: self.pause_health.clone(),
                health_started: self.health_started.clone(),
                health_release: self.health_release.clone(),
                dropped: self.dropped.clone(),
            }) as Arc<dyn ModelInstance>)
        })
    }
}

struct FakeModelInstance {
    identity: ModelIdentity,
    runtime: String,
    capabilities: Vec<String>,
    behavior: FakeRuntimeBehavior,
    started: Arc<AtomicUsize>,
    release: Arc<Notify>,
    unhealthy_models: Arc<RwLock<HashSet<String>>>,
    pause_health: Arc<AtomicBool>,
    health_started: Arc<AtomicUsize>,
    health_release: Arc<Notify>,
    dropped: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for FakeModelInstance {
    fn drop(&mut self) {
        let mut dropped = self.dropped.lock().expect("drop state poisoned");
        *dropped.entry(self.identity.model_id.clone()).or_default() += 1;
    }
}

impl ModelInstance for FakeModelInstance {
    fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    fn runtime(&self) -> &str {
        &self.runtime
    }

    fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    fn self_test<'a>(&'a self, _cases: &'a [SelfTestCase]) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            if self.behavior.fail_self_test
                || self
                    .behavior
                    .fail_self_test_model
                    .as_ref()
                    .is_some_and(|model_id| model_id == &self.identity.model_id)
            {
                Err(ModelError::new(
                    "model_self_test_failed",
                    "fake runtime self-test failure",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn health<'a>(&'a self) -> RuntimeFuture<'a, ()> {
        Box::pin(async move {
            let released = self.health_release.notified();
            self.health_started.fetch_add(1, Ordering::AcqRel);
            if self.pause_health.load(Ordering::Acquire) {
                released.await;
            }
            if self.behavior.fail_health
                || self
                    .unhealthy_models
                    .read()
                    .expect("health state poisoned")
                    .contains(&self.identity.model_id)
            {
                Err(ModelError::new(
                    "model_health_failed",
                    "fake runtime health failure",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn infer<'a>(&'a self, input: Vec<u8>) -> RuntimeFuture<'a, InferenceResult> {
        Box::pin(async move {
            self.started.fetch_add(1, Ordering::AcqRel);
            if self.behavior.block_inference {
                self.release.notified().await;
            }
            Ok(InferenceResult {
                output: input,
                actual_model: self.identity.actual_model(self.runtime.clone()),
            })
        })
    }
}
