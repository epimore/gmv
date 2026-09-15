mod manager;
mod onnx_cpu;
mod package;
mod repository;
mod runtime;

pub use manager::{
    ActiveModel, HealthReconcile, ModelManager, ModelManagerConfig, ModelObservation, ModelStatus,
    RecoveredCapability,
};
#[cfg(any(test, feature = "native-onnx-tests"))]
pub use onnx_cpu::NativeRuntimeSnapshot;
pub use onnx_cpu::{ONNX_CPU_RUNTIME, ONNX_RUNTIME_VERSION, OnnxCpuConfig, OnnxCpuProvider};
pub use package::{
    ExecutionContract, ExecutionInput, ExecutionLimits, LicenseSpec, ModelFile, ModelIdentity,
    ModelPackageManifest, PackagePolicy, PostprocessContract, PreprocessContract, ResourceHints,
    ResultSchema, RuntimeVariant, SelectedRuntimeVariant, SelfTestCase, SelfTestOracle,
    SigningSpec, TensorContract, VerifiedModelPackage, model_package_signing_payload,
    verify_package,
};
pub(crate) use repository::{
    ClaimOperation, OperationClaimRequest, OperationReceipt, OperationReceiptLimits,
    OperationReceiptState,
};
pub use repository::{InstalledModel, ModelRepository, ModelState};
#[cfg(test)]
pub use runtime::{FakeRuntimeBehavior, FakeRuntimeProvider};
pub use runtime::{
    InferenceResult, ModelInstance, RuntimeCallContext, RuntimeDescriptor, RuntimeInput,
    RuntimeProvider,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub code: &'static str,
    pub message: String,
}

impl ModelError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn io(stage: &'static str, error: impl std::fmt::Display) -> Self {
        Self::new("model_io_failed", format!("{stage}: {error}"))
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ModelError {}

pub type ModelResult<T> = Result<T, ModelError>;
