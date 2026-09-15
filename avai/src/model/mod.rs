mod manager;
mod package;
mod repository;
mod runtime;

pub use manager::{
    ActiveModel, HealthReconcile, ModelManager, ModelManagerConfig, ModelObservation, ModelStatus,
    RecoveredCapability,
};
pub use package::{
    LicenseSpec, ModelFile, ModelIdentity, ModelPackageManifest, PackagePolicy, ResourceHints,
    ResultSchema, RuntimeVariant, SelfTestCase, SigningSpec, VerifiedModelPackage,
    model_package_signing_payload, verify_package,
};
pub(crate) use repository::{
    ClaimOperation, OperationClaimRequest, OperationReceipt, OperationReceiptLimits,
    OperationReceiptState,
};
pub use repository::{InstalledModel, ModelRepository, ModelState};
#[cfg(test)]
pub use runtime::{FakeRuntimeBehavior, FakeRuntimeProvider};
pub use runtime::{InferenceResult, ModelInstance, RuntimeDescriptor, RuntimeProvider};

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
