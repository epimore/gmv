use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
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
        }
    }

    pub fn started_inferences(&self) -> usize {
        self.started.load(Ordering::Acquire)
    }

    pub fn release_inferences(&self) {
        self.release.notify_waiters();
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
            if self.behavior.fail_health {
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
