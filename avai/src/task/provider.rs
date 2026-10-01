use super::*;

pub(super) trait ImageInferenceProvider: Send + Sync {
    fn manifest(&self) -> ProviderManifest;

    fn infer<'a>(
        &'a self,
        capability: &'a str,
        image: ResolvedImage,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<InferenceOutput, TaskError>> + Send + 'a>,
    >;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderManifest {
    pub(super) capability: &'static str,
    pub(super) model_id: &'static str,
    pub(super) model_version: &'static str,
    pub(super) runtime: &'static str,
    pub(super) result_schema: &'static str,
    pub(super) result_schema_version: u32,
}

pub(super) struct ProviderRegistry {
    providers: Vec<Arc<dyn ImageInferenceProvider>>,
    model_manager: Option<ModelManager>,
    max_result_bytes: usize,
}

impl ProviderRegistry {
    pub(super) fn new(
        configured_capabilities: &[String],
        model_manager: Option<ModelManager>,
        max_result_bytes: usize,
    ) -> Result<Self, TaskError> {
        let providers: Vec<Arc<dyn ImageInferenceProvider>> =
            vec![Arc::new(BuiltinImageMetadataProvider)];
        let builtin = providers
            .iter()
            .map(|provider| provider.manifest().capability)
            .collect::<HashSet<_>>();
        if model_manager.is_none()
            && let Some(capability) = configured_capabilities
                .iter()
                .find(|capability| !builtin.contains(capability.as_str()))
        {
            return Err(TaskError::new(
                "invalid_task_config",
                format!("configured capability has no installed provider: {capability}"),
            ));
        }
        Ok(Self {
            providers,
            model_manager,
            max_result_bytes,
        })
    }

    pub(super) async fn capture(
        &self,
        capability: &str,
        requested_model: Option<&ModelRef>,
        durable_binding: Option<&ExecutionBinding>,
    ) -> Result<CapturedExecution, TaskError> {
        if let Some(binding) = durable_binding {
            return self.capture_bound(capability, binding).await;
        }
        let builtin = self
            .providers
            .iter()
            .find(|provider| provider.manifest().capability == capability)
            .cloned();
        let managed_active = if let Some(manager) = &self.model_manager {
            manager.has_active(capability).await
        } else {
            false
        };
        if builtin.is_some() && managed_active {
            return Err(TaskError::new(
                "model_selection_conflict",
                "builtin and managed model both own the requested capability",
            ));
        }
        if let Some(requested) = requested_model {
            if requested.revision.is_empty() {
                if let Some(provider) = builtin.filter(|provider| {
                    let manifest = provider.manifest();
                    requested.model_id == manifest.model_id
                        && requested.version == manifest.model_version
                        && (requested.runtime.is_empty() || requested.runtime == manifest.runtime)
                }) {
                    return Ok(CapturedExecution::builtin(provider));
                }
                return Err(TaskError::new(
                    "model_revision_required",
                    "managed model requests require an immutable revision",
                ));
            }
            let manager = self.model_manager.as_ref().ok_or_else(|| {
                TaskError::new("model_not_found", "managed model runtime is unavailable")
            })?;
            let identity = ModelIdentity {
                model_id: requested.model_id.clone(),
                version: requested.version.clone(),
                revision: requested.revision.clone(),
            };
            let model = manager
                .capture_exact(capability, &identity, &requested.runtime)
                .await
                .map_err(model_task_error)?;
            return Ok(CapturedExecution::managed(
                model,
                capability,
                self.max_result_bytes,
            ));
        }
        if managed_active {
            let model = self
                .model_manager
                .as_ref()
                .expect("managed_active requires manager")
                .capture(capability)
                .await
                .map_err(model_task_error)?;
            return Ok(CapturedExecution::managed(
                model,
                capability,
                self.max_result_bytes,
            ));
        }
        builtin
            .map(CapturedExecution::builtin)
            .ok_or_else(|| TaskError::new("model_not_found", "no execution owns the capability"))
    }

    async fn capture_bound(
        &self,
        capability: &str,
        binding: &ExecutionBinding,
    ) -> Result<CapturedExecution, TaskError> {
        if binding.capability != capability {
            return Err(TaskError::new(
                "invalid_execution_binding",
                "durable execution binding capability does not match the task",
            ));
        }
        match binding.kind {
            ExecutionKind::Builtin => {
                let provider = self
                    .providers
                    .iter()
                    .find(|provider| binding.matches_manifest(provider.manifest()))
                    .cloned()
                    .ok_or_else(|| {
                        TaskError::new(
                            "bound_model_unavailable",
                            "bound builtin execution is unavailable",
                        )
                    })?;
                Ok(CapturedExecution::Builtin {
                    provider,
                    binding: binding.clone(),
                })
            }
            ExecutionKind::Managed => {
                let manager = self.model_manager.as_ref().ok_or_else(|| {
                    TaskError::new(
                        "bound_model_unavailable",
                        "bound model runtime is unavailable",
                    )
                })?;
                let identity = ModelIdentity {
                    model_id: binding.model_id.clone(),
                    version: binding.model_version.clone(),
                    revision: binding.revision.clone(),
                };
                let model = manager
                    .capture_recovered(
                        capability,
                        &identity,
                        &binding.runtime,
                        &binding.result_schema_name,
                        binding.result_schema_version,
                        now_epoch_ms(),
                    )
                    .await
                    .map_err(|_| {
                        TaskError::new(
                            "bound_model_unavailable",
                            "bound model revision cannot be safely recovered",
                        )
                    })?;
                Ok(CapturedExecution::Managed {
                    model,
                    binding: binding.clone(),
                    max_result_bytes: self.max_result_bytes,
                })
            }
        }
    }
}

pub(super) struct BuiltinImageMetadataProvider;

impl ImageInferenceProvider for BuiltinImageMetadataProvider {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            capability: BUILTIN_CAPABILITY,
            model_id: "builtin.image-metadata",
            model_version: "1",
            runtime: "rust-image",
            result_schema: "gmv.image.metadata.inspect",
            result_schema_version: 1,
        }
    }

    fn infer<'a>(
        &'a self,
        capability: &'a str,
        image: ResolvedImage,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<InferenceOutput, TaskError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let manifest = self.manifest();
            if capability != manifest.capability {
                return Err(TaskError::new(
                    "model_not_found",
                    "no installed inference provider implements the requested capability",
                ));
            }
            let payload = base::serde_json::to_vec(&base::serde_json::json!({
                "content_type": image.content_type,
                "size_bytes": image.bytes.len(),
                "sha256": image.sha256,
                "width": image.width,
                "height": image.height,
                "source_identity": image.source_identity,
            }))
            .map_err(|error| TaskError::internal("encode_result", error))?;
            Ok(InferenceOutput {
                result: AiTaskResult {
                    output: Some(VersionedPayload {
                        schema: manifest.result_schema.to_string(),
                        version: manifest.result_schema_version,
                        json: payload,
                    }),
                    actual_model: Some(ModelRef {
                        model_id: manifest.model_id.to_string(),
                        version: manifest.model_version.to_string(),
                        runtime: manifest.runtime.to_string(),
                        revision: String::new(),
                    }),
                    evidence: Vec::new(),
                    completed_at_epoch_ms: now_epoch_ms(),
                },
            })
        })
    }
}

pub(super) enum CapturedExecution {
    Builtin {
        provider: Arc<dyn ImageInferenceProvider>,
        binding: ExecutionBinding,
    },
    Managed {
        model: ActiveModel,
        binding: ExecutionBinding,
        max_result_bytes: usize,
    },
}

impl CapturedExecution {
    fn builtin(provider: Arc<dyn ImageInferenceProvider>) -> Self {
        let binding = ExecutionBinding::builtin(provider.manifest());
        Self::Builtin { provider, binding }
    }

    fn managed(model: ActiveModel, capability: &str, max_result_bytes: usize) -> Self {
        let binding = ExecutionBinding::managed(&model, capability);
        Self::Managed {
            model,
            binding,
            max_result_bytes,
        }
    }

    pub(super) fn binding(&self) -> &ExecutionBinding {
        match self {
            Self::Builtin { binding, .. } | Self::Managed { binding, .. } => binding,
        }
    }

    pub(super) fn generation(&self) -> Option<u64> {
        match self {
            Self::Builtin { .. } => None,
            Self::Managed { model, .. } => Some(model.generation()),
        }
    }

    pub(super) async fn infer(
        &self,
        capability: &str,
        image: ResolvedImage,
        context: RuntimeCallContext,
    ) -> Result<InferenceOutput, TaskError> {
        match self {
            Self::Builtin { provider, .. } => provider.infer(capability, image).await,
            Self::Managed {
                model,
                binding,
                max_result_bytes,
            } => {
                let output = model
                    .infer(
                        RuntimeInput {
                            encoded: image.bytes,
                            media_type: image.content_type,
                            width: image.width,
                            height: image.height,
                        },
                        context,
                    )
                    .await
                    .map_err(model_task_error)?;
                let expected = ModelRef {
                    model_id: binding.model_id.clone(),
                    version: binding.model_version.clone(),
                    runtime: binding.runtime.clone(),
                    revision: binding.revision.clone(),
                };
                if output.actual_model != expected {
                    return Err(TaskError::new(
                        "model_runtime_contract_mismatch",
                        "runtime reported a different model identity",
                    ));
                }
                if output.output.len() > *max_result_bytes {
                    return Err(TaskError::new(
                        "result_too_large",
                        "model result exceeds the configured limit",
                    ));
                }
                base::serde_json::from_slice::<base::serde_json::Value>(&output.output).map_err(
                    |_| TaskError::new("invalid_result_json", "model result is not valid JSON"),
                )?;
                Ok(InferenceOutput {
                    result: AiTaskResult {
                        output: Some(VersionedPayload {
                            schema: binding.result_schema_name.clone(),
                            version: binding.result_schema_version,
                            json: output.output,
                        }),
                        actual_model: Some(expected),
                        evidence: Vec::new(),
                        completed_at_epoch_ms: now_epoch_ms(),
                    },
                })
            }
        }
    }
}

fn model_task_error(error: ModelError) -> TaskError {
    let (code, message) = match error.code {
        "model_not_found" => ("model_not_found", "requested model revision was not found"),
        "model_not_ready" => ("model_not_ready", "requested model is not ready"),
        "model_failed" => ("model_failed", "requested model has failed"),
        "model_runtime_incompatible" => (
            "model_runtime_incompatible",
            "requested model runtime is incompatible",
        ),
        "model_capability_incompatible" => (
            "model_capability_incompatible",
            "requested model capability is incompatible",
        ),
        "model_not_active" => ("model_not_ready", "no active model is ready"),
        _ => ("model_execution_failed", "managed model execution failed"),
    };
    TaskError::new(code, message)
}
