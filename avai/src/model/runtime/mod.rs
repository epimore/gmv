use std::{future::Future, pin::Pin, sync::Arc, time::Instant};

use base::{bytes::Bytes, tokio_util::sync::CancellationToken};

use super::{
    ExecutionContract, InstalledModel, ModelError, ModelIdentity, ModelResult, RuntimeVariant,
    SelfTestCase,
};

pub type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = ModelResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDescriptor {
    pub runtime: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeCallContext {
    pub deadline: Instant,
    pub cancellation: CancellationToken,
}

impl RuntimeCallContext {
    pub fn local(maximum: std::time::Duration, cancellation: CancellationToken) -> Self {
        Self {
            deadline: Instant::now() + maximum,
            cancellation,
        }
    }

    pub fn with_local_maximum(&self, maximum: std::time::Duration) -> Self {
        Self {
            deadline: self.deadline.min(Instant::now() + maximum),
            cancellation: self.cancellation.clone(),
        }
    }

    pub fn ensure_active(&self) -> ModelResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(ModelError::new(
                "model_runtime_cancelled",
                "runtime call was cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ModelError::new(
                "model_runtime_deadline_exceeded",
                "runtime call deadline expired",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeInput {
    pub encoded: Bytes,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
}

pub trait RuntimeProvider: Send + Sync {
    fn descriptor(&self) -> RuntimeDescriptor;

    fn validate_selector(&self, selector: &RuntimeVariant) -> ModelResult<()>;

    fn preload<'a>(
        &'a self,
        model: &'a InstalledModel,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, Arc<dyn ModelInstance>>;
}

pub trait ModelInstance: Send + Sync {
    fn identity(&self) -> &ModelIdentity;
    fn runtime(&self) -> &str;
    fn capabilities(&self) -> &[String];

    fn unload<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()>;

    fn self_test<'a>(
        &'a self,
        cases: &'a [SelfTestCase],
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, ()>;

    fn health<'a>(&'a self, context: RuntimeCallContext) -> RuntimeFuture<'a, ()>;

    fn infer<'a>(
        &'a self,
        input: RuntimeInput,
        context: RuntimeCallContext,
    ) -> RuntimeFuture<'a, InferenceResult>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceResult {
    pub output: Vec<u8>,
    pub actual_model: gmv_protocol::avai::v1::ModelRef,
}

pub(crate) fn compare_json_numeric(
    actual: &[u8],
    expected: &[u8],
    oracle: &super::SelfTestOracle,
) -> ModelResult<()> {
    let actual: base::serde_json::Value = base::serde_json::from_slice(actual)
        .map_err(|error| ModelError::new("model_runtime_response_invalid", error.to_string()))?;
    let expected: base::serde_json::Value = base::serde_json::from_slice(expected)
        .map_err(|error| ModelError::new("model_self_test_failed", error.to_string()))?;
    if json_numeric_equal(
        &actual,
        &expected,
        oracle.abs_tolerance,
        oracle.rel_tolerance,
    ) {
        Ok(())
    } else {
        Err(ModelError::new(
            "model_self_test_failed",
            "model output does not match the signed numeric oracle",
        ))
    }
}

fn json_numeric_equal(
    actual: &base::serde_json::Value,
    expected: &base::serde_json::Value,
    abs_tolerance: f64,
    rel_tolerance: f64,
) -> bool {
    match (actual, expected) {
        (base::serde_json::Value::Number(actual), base::serde_json::Value::Number(expected)) => {
            let (Some(actual), Some(expected)) = (actual.as_f64(), expected.as_f64()) else {
                return false;
            };
            actual.is_finite()
                && expected.is_finite()
                && (actual - expected).abs() <= abs_tolerance + rel_tolerance * expected.abs()
        }
        (base::serde_json::Value::Array(actual), base::serde_json::Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(actual, expected)| {
                    json_numeric_equal(actual, expected, abs_tolerance, rel_tolerance)
                })
        }
        (base::serde_json::Value::Object(actual), base::serde_json::Value::Object(expected)) => {
            actual.len() == expected.len()
                && actual.iter().all(|(key, actual)| {
                    expected.get(key).is_some_and(|expected| {
                        json_numeric_equal(actual, expected, abs_tolerance, rel_tolerance)
                    })
                })
        }
        _ => actual == expected,
    }
}

pub(crate) fn validate_tensor_json(
    encoded: &[u8],
    execution: &ExecutionContract,
) -> ModelResult<()> {
    let value: base::serde_json::Value = base::serde_json::from_slice(encoded)
        .map_err(|error| ModelError::new("model_runtime_response_invalid", error.to_string()))?;
    let outputs = value
        .as_object()
        .and_then(|root| (root.len() == 1).then(|| root.get("outputs")).flatten())
        .and_then(base::serde_json::Value::as_array)
        .ok_or_else(|| {
            ModelError::new(
                "model_runtime_response_invalid",
                "provider result is not tensor_json_v1",
            )
        })?;
    if outputs.len() != execution.outputs.len() {
        return Err(ModelError::new(
            "model_runtime_response_invalid",
            "provider result tensor count does not match the signed contract",
        ));
    }
    for (actual, expected) in outputs.iter().zip(&execution.outputs) {
        let actual = actual.as_object().ok_or_else(|| {
            ModelError::new(
                "model_runtime_response_invalid",
                "provider tensor result is not an object",
            )
        })?;
        let shape = actual
            .get("shape")
            .and_then(base::serde_json::Value::as_array)
            .ok_or_else(|| {
                ModelError::new(
                    "model_runtime_response_invalid",
                    "provider shape is invalid",
                )
            })?;
        let data = actual
            .get("data")
            .and_then(base::serde_json::Value::as_array)
            .ok_or_else(|| {
                ModelError::new("model_runtime_response_invalid", "provider data is invalid")
            })?;
        let expected_elements = expected
            .shape
            .iter()
            .try_fold(1_u64, |count, dimension| count.checked_mul(*dimension));
        if actual.len() != 4
            || actual.get("name").and_then(base::serde_json::Value::as_str)
                != Some(expected.name.as_str())
            || actual
                .get("dtype")
                .and_then(base::serde_json::Value::as_str)
                != Some("f32")
            || shape.len() != expected.shape.len()
            || !shape
                .iter()
                .zip(&expected.shape)
                .all(|(actual, expected)| actual.as_u64().is_some_and(|actual| actual == *expected))
            || expected_elements.is_none_or(|count| data.len() as u64 != count)
            || data
                .iter()
                .any(|value| value.as_f64().is_none_or(|value| !value.is_finite()))
        {
            return Err(ModelError::new(
                "model_runtime_response_invalid",
                "provider tensor result does not match the signed contract",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/model/runtime.rs"]
mod test_support;
#[cfg(test)]
pub use test_support::{FakeRuntimeBehavior, FakeRuntimeProvider};
