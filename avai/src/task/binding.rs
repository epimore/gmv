use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub(super) enum ExecutionKind {
    Builtin,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub(super) struct ExecutionBinding {
    pub(super) version: u32,
    pub(super) kind: ExecutionKind,
    pub(super) capability: String,
    pub(super) model_id: String,
    pub(super) model_version: String,
    pub(super) revision: String,
    pub(super) runtime: String,
    pub(super) result_schema_name: String,
    pub(super) result_schema_version: u32,
}

impl ExecutionBinding {
    const VERSION: u32 = 1;

    pub(super) fn builtin(manifest: ProviderManifest) -> Self {
        Self {
            version: Self::VERSION,
            kind: ExecutionKind::Builtin,
            capability: manifest.capability.to_string(),
            model_id: manifest.model_id.to_string(),
            model_version: manifest.model_version.to_string(),
            revision: String::new(),
            runtime: manifest.runtime.to_string(),
            result_schema_name: manifest.result_schema.to_string(),
            result_schema_version: manifest.result_schema_version,
        }
    }

    pub(super) fn managed(model: &ActiveModel, capability: &str) -> Self {
        Self {
            version: Self::VERSION,
            kind: ExecutionKind::Managed,
            capability: capability.to_string(),
            model_id: model.identity().model_id.clone(),
            model_version: model.identity().version.clone(),
            revision: model.identity().revision.clone(),
            runtime: model.runtime().to_string(),
            result_schema_name: model.result_schema().name.clone(),
            result_schema_version: model.result_schema().version,
        }
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, TaskError> {
        base::serde_json::to_vec(self)
            .map_err(|error| TaskError::internal("encode_execution_binding", error))
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, TaskError> {
        let binding: Self = base::serde_json::from_slice(bytes).map_err(|_| {
            TaskError::new(
                "invalid_execution_binding",
                "durable execution binding is invalid",
            )
        })?;
        if binding.version != Self::VERSION {
            return Err(TaskError::new(
                "invalid_execution_binding",
                "durable execution binding version is unsupported",
            ));
        }
        Ok(binding)
    }

    pub(super) fn matches_manifest(&self, manifest: ProviderManifest) -> bool {
        self.version == Self::VERSION
            && self.kind == ExecutionKind::Builtin
            && self.capability == manifest.capability
            && self.model_id == manifest.model_id
            && self.model_version == manifest.model_version
            && self.revision.is_empty()
            && self.runtime == manifest.runtime
            && self.result_schema_name == manifest.result_schema
            && self.result_schema_version == manifest.result_schema_version
    }

    pub(super) fn metric_identity(&self) -> ActualModelIdentity {
        ActualModelIdentity {
            model_id: self.model_id.clone(),
            version: self.model_version.clone(),
            revision: self.revision.clone(),
            runtime: self.runtime.clone(),
        }
    }
}
