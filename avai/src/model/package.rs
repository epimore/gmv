use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use base::{
    base64::Engine,
    serde::{Deserialize, Serialize},
    sha2::{Digest, Sha256},
};

use super::{InstalledModel, ModelError, ModelResult};

const MANIFEST_FILE: &str = "manifest.yaml";
const API_VERSION: &str = "gmv.ai/v1";
const PACKAGE_KIND: &str = "ModelPlugin";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ModelIdentity {
    pub model_id: String,
    pub version: String,
    pub revision: String,
}

impl ModelIdentity {
    pub fn actual_model(&self, runtime: impl Into<String>) -> gmv_protocol::avai::v1::ModelRef {
        gmv_protocol::avai::v1::ModelRef {
            model_id: self.model_id.clone(),
            version: self.version.clone(),
            runtime: runtime.into(),
            revision: self.revision.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ModelPackageManifest {
    pub api_version: String,
    pub kind: String,
    pub metadata: ModelIdentity,
    pub capabilities: Vec<String>,
    pub result_schema: ResultSchema,
    pub variants: Vec<RuntimeVariant>,
    #[serde(default)]
    pub execution: Option<ExecutionContract>,
    pub resources: ResourceHints,
    pub license: LicenseSpec,
    #[serde(default)]
    pub self_test: Vec<SelfTestCase>,
    pub files: Vec<ModelFile>,
    pub signing: SigningSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ResultSchema {
    pub name: String,
    pub version: u32,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct RuntimeVariant {
    pub runtime: String,
    #[serde(default)]
    pub runtime_contract_version: u32,
    pub architecture: String,
    #[serde(default)]
    pub accelerator: String,
    pub artifact: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ExecutionContract {
    pub version: u32,
    pub input: ExecutionInput,
    pub outputs: Vec<TensorContract>,
    pub postprocess: PostprocessContract,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ExecutionInput {
    pub kind: String,
    pub accepted_media_types: Vec<String>,
    pub max_bytes: u64,
    pub max_width: u32,
    pub max_height: u32,
    pub tensor: TensorContract,
    pub preprocess: PreprocessContract,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct TensorContract {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<u64>,
    #[serde(default)]
    pub layout: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct PreprocessContract {
    pub resize: String,
    pub interpolation: String,
    pub color: String,
    pub scale: f32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct PostprocessContract {
    pub kind: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ResourceHints {
    pub memory_mb: u64,
    #[serde(default)]
    pub vram_mb: u64,
    pub max_batch: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct LicenseSpec {
    pub spdx: String,
    pub commercial_use: bool,
    pub redistribution: String,
    #[serde(default)]
    pub license_ref: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SelfTestCase {
    pub input: String,
    pub expected: String,
    #[serde(default)]
    pub oracle: Option<SelfTestOracle>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SelfTestOracle {
    pub kind: String,
    pub abs_tolerance: f64,
    pub rel_tolerance: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ModelFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SigningSpec {
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct PackagePolicy {
    pub architecture: String,
    pub available_runtimes: HashSet<String>,
    pub available_accelerators: HashSet<String>,
    pub allowed_result_schemas: HashSet<(String, u32)>,
    pub approved_spdx: HashSet<String>,
    pub available_license_refs: HashSet<String>,
    pub trusted_signing_keys: HashMap<String, Vec<u8>>,
    pub max_manifest_bytes: u64,
    pub max_file_count: usize,
    pub max_package_bytes: u64,
    pub max_memory_mb: u64,
    pub max_vram_mb: u64,
    pub execution_limits: ExecutionLimits,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionLimits {
    pub max_input_elements: usize,
    pub max_input_bytes: usize,
    pub max_output_tensors: usize,
    pub max_output_elements: usize,
    pub max_output_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_input_elements: 16 * 1024 * 1024,
            max_input_bytes: 64 * 1024 * 1024,
            max_output_tensors: 16,
            max_output_elements: 16 * 1024 * 1024,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

impl Default for PackagePolicy {
    fn default() -> Self {
        Self {
            architecture: std::env::consts::ARCH.to_string(),
            available_runtimes: HashSet::new(),
            available_accelerators: HashSet::from(["cpu".to_string()]),
            allowed_result_schemas: HashSet::new(),
            approved_spdx: HashSet::new(),
            available_license_refs: HashSet::new(),
            trusted_signing_keys: HashMap::new(),
            max_manifest_bytes: 256 * 1024,
            max_file_count: 256,
            max_package_bytes: 4 * 1024 * 1024 * 1024,
            max_memory_mb: 16 * 1024,
            max_vram_mb: 16 * 1024,
            execution_limits: ExecutionLimits::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedModelPackage {
    pub root: PathBuf,
    pub manifest: ModelPackageManifest,
    pub selected_variant: RuntimeVariant,
    pub manifest_sha256: String,
    policy: PackagePolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct InstalledExecutionContract {
    pub result_schema: ResultSchema,
    pub selected_variant: SelectedRuntimeVariant,
    pub execution: Option<ExecutionContract>,
    pub self_tests: Vec<SelfTestCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SelectedRuntimeVariant {
    pub version: u32,
    pub runtime: String,
    pub runtime_contract_version: u32,
    pub architecture: String,
    pub accelerator: String,
    pub artifact: String,
    pub artifact_sha256: String,
}

impl SelectedRuntimeVariant {
    pub(crate) fn from_verified(package: &VerifiedModelPackage) -> ModelResult<Self> {
        let artifact = safe_relative_path(&package.selected_variant.artifact)?;
        let file = package
            .manifest
            .files
            .iter()
            .find(|file| safe_relative_path(&file.path).is_ok_and(|path| path == artifact))
            .ok_or_else(|| {
                ModelError::new(
                    "invalid_model_manifest",
                    "selected artifact is not hash-declared",
                )
            })?;
        Ok(Self {
            version: 1,
            runtime: package.selected_variant.runtime.clone(),
            runtime_contract_version: package.selected_variant.runtime_contract_version,
            architecture: package.selected_variant.architecture.clone(),
            accelerator: package.selected_variant.accelerator.clone(),
            artifact: package.selected_variant.artifact.clone(),
            artifact_sha256: file
                .sha256
                .trim_start_matches("sha256:")
                .to_ascii_lowercase(),
        })
    }
}

pub(crate) fn load_installed_execution_contract(
    model: &InstalledModel,
) -> ModelResult<InstalledExecutionContract> {
    let manifest_path = model.installed_path.join(MANIFEST_FILE);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|error| ModelError::io("read installed model manifest", error))?;
    let manifest_hash = format!("{:x}", Sha256::digest(&manifest_bytes));
    if manifest_hash != model.manifest_sha256 {
        return Err(ModelError::new(
            "model_package_changed",
            "installed model manifest no longer matches its verified hash",
        ));
    }
    let manifest: ModelPackageManifest = base::serde_yaml::from_slice(&manifest_bytes)
        .map_err(|error| ModelError::new("invalid_model_manifest", error.to_string()))?;
    if manifest.metadata != model.identity || manifest.capabilities != model.capabilities {
        return Err(ModelError::new(
            "model_package_changed",
            "installed model execution metadata no longer matches durable metadata",
        ));
    }
    let selected_variant = match &model.selected_variant {
        Some(selected) => {
            if selected.version != 1 || selected.runtime != model.runtime {
                return Err(ModelError::new(
                    "model_selected_variant_invalid",
                    "persisted selected variant is invalid",
                ));
            }
            let matches = manifest.variants.iter().any(|variant| {
                variant.runtime == selected.runtime
                    && variant.runtime_contract_version == selected.runtime_contract_version
                    && variant.architecture == selected.architecture
                    && variant.accelerator == selected.accelerator
                    && variant.artifact == selected.artifact
            });
            if !matches {
                return Err(ModelError::new(
                    "model_package_changed",
                    "persisted selected variant no longer matches the immutable manifest",
                ));
            }
            selected.clone()
        }
        None => {
            let mut matches = manifest
                .variants
                .iter()
                .filter(|variant| variant.runtime == model.runtime);
            let variant = matches.next().ok_or_else(|| {
                ModelError::new(
                    "model_selected_variant_missing",
                    "legacy model has no matching runtime variant",
                )
            })?;
            if matches.next().is_some() {
                return Err(ModelError::new(
                    "model_selected_variant_ambiguous",
                    "legacy model runtime maps to multiple manifest variants",
                ));
            }
            selected_from_manifest(&manifest, variant)?
        }
    };
    verify_installed_file(
        model,
        &manifest,
        &selected_variant.artifact,
        Some(&selected_variant.artifact_sha256),
        "selected model artifact",
    )?;
    let schema_path = safe_relative_path(&manifest.result_schema.path)?;
    let declared = manifest
        .files
        .iter()
        .find(|file| safe_relative_path(&file.path).is_ok_and(|path| path == schema_path))
        .ok_or_else(|| {
            ModelError::new(
                "model_package_changed",
                "installed result schema is not hash-declared",
            )
        })?;
    let schema_bytes = std::fs::read(model.installed_path.join(&schema_path))
        .map_err(|error| ModelError::io("read installed result schema", error))?;
    if schema_bytes.len() as u64 != declared.size
        || !format!("{:x}", Sha256::digest(&schema_bytes))
            .eq_ignore_ascii_case(declared.sha256.trim_start_matches("sha256:"))
    {
        return Err(ModelError::new(
            "model_package_changed",
            "installed result schema no longer matches its declared hash",
        ));
    }
    base::serde_json::from_slice::<base::serde_json::Value>(&schema_bytes).map_err(|error| {
        ModelError::new(
            "invalid_result_schema",
            format!("installed result schema is invalid JSON: {error}"),
        )
    })?;
    Ok(InstalledExecutionContract {
        result_schema: manifest.result_schema,
        selected_variant,
        execution: manifest.execution,
        self_tests: manifest.self_test,
    })
}

fn selected_from_manifest(
    manifest: &ModelPackageManifest,
    variant: &RuntimeVariant,
) -> ModelResult<SelectedRuntimeVariant> {
    let artifact = safe_relative_path(&variant.artifact)?;
    let file = manifest
        .files
        .iter()
        .find(|file| safe_relative_path(&file.path).is_ok_and(|path| path == artifact))
        .ok_or_else(|| {
            ModelError::new(
                "model_package_changed",
                "selected model artifact is not hash-declared",
            )
        })?;
    Ok(SelectedRuntimeVariant {
        version: 1,
        runtime: variant.runtime.clone(),
        runtime_contract_version: variant.runtime_contract_version,
        architecture: variant.architecture.clone(),
        accelerator: variant.accelerator.clone(),
        artifact: variant.artifact.clone(),
        artifact_sha256: file
            .sha256
            .trim_start_matches("sha256:")
            .to_ascii_lowercase(),
    })
}

fn verify_installed_file(
    model: &InstalledModel,
    manifest: &ModelPackageManifest,
    relative: &str,
    expected_hash: Option<&str>,
    description: &str,
) -> ModelResult<()> {
    let relative = safe_relative_path(relative)?;
    let declared = manifest
        .files
        .iter()
        .find(|file| safe_relative_path(&file.path).is_ok_and(|path| path == relative))
        .ok_or_else(|| {
            ModelError::new(
                "model_package_changed",
                format!("installed {description} is not hash-declared"),
            )
        })?;
    if expected_hash.is_some_and(|hash| {
        !hash.eq_ignore_ascii_case(declared.sha256.trim_start_matches("sha256:"))
    }) {
        return Err(ModelError::new(
            "model_package_changed",
            format!("installed {description} hash does not match selected variant"),
        ));
    }
    let metadata = confined_regular_file_metadata(&model.installed_path, &relative)?;
    let bytes = std::fs::read(model.installed_path.join(relative)).map_err(|error| {
        ModelError::new(
            "model_io_failed",
            format!("read installed {description}: {error}"),
        )
    })?;
    if metadata.len() != declared.size
        || !format!("{:x}", Sha256::digest(&bytes))
            .eq_ignore_ascii_case(declared.sha256.trim_start_matches("sha256:"))
    {
        return Err(ModelError::new(
            "model_package_changed",
            format!("installed {description} no longer matches its declared hash"),
        ));
    }
    Ok(())
}

impl VerifiedModelPackage {
    pub(crate) fn verify_staged_copy(&self, root: &Path) -> ModelResult<()> {
        let staged = verify_package(root, &self.policy)?;
        if staged.manifest.metadata != self.manifest.metadata
            || staged.manifest_sha256 != self.manifest_sha256
            || staged.selected_variant.runtime != self.selected_variant.runtime
            || staged.selected_variant.artifact != self.selected_variant.artifact
        {
            return Err(ModelError::new(
                "model_package_changed",
                "model package changed after verification",
            ));
        }
        Ok(())
    }
}

pub fn verify_package(root: &Path, policy: &PackagePolicy) -> ModelResult<VerifiedModelPackage> {
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|error| ModelError::io("inspect package root", error))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(ModelError::new(
            "invalid_model_package",
            "package root must be a real directory",
        ));
    }
    let manifest_path = root.join(MANIFEST_FILE);
    let manifest_metadata = regular_file_metadata(&manifest_path)?;
    if manifest_metadata.len() > policy.max_manifest_bytes {
        return Err(ModelError::new(
            "model_package_too_large",
            "manifest exceeds configured size limit",
        ));
    }
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|error| ModelError::io("read model manifest", error))?;
    let manifest: ModelPackageManifest = base::serde_yaml::from_slice(&manifest_bytes)
        .map_err(|error| ModelError::new("invalid_model_manifest", error.to_string()))?;
    validate_manifest(&manifest, policy)?;
    verify_signature(&manifest, policy)?;
    if manifest.files.len() > policy.max_file_count {
        return Err(ModelError::new(
            "model_package_too_large",
            "package contains too many files",
        ));
    }

    let mut paths = HashSet::new();
    let mut total_bytes = manifest_metadata.len();
    for file in &manifest.files {
        let relative = safe_relative_path(&file.path)?;
        if !paths.insert(relative.clone()) {
            return Err(ModelError::new(
                "invalid_model_manifest",
                format!("duplicate file entry: {}", file.path),
            ));
        }
        let path = root.join(&relative);
        let metadata = confined_regular_file_metadata(root, &relative)?;
        if metadata.len() != file.size {
            return Err(ModelError::new(
                "model_file_size_mismatch",
                format!("model file size mismatch: {}", file.path),
            ));
        }
        total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
            ModelError::new("model_package_too_large", "package byte count overflow")
        })?;
        if total_bytes > policy.max_package_bytes {
            return Err(ModelError::new(
                "model_package_too_large",
                "package exceeds configured byte limit",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| ModelError::io("read model package file", error))?;
        let actual = format!("{:x}", Sha256::digest(bytes));
        if !actual.eq_ignore_ascii_case(file.sha256.trim_start_matches("sha256:")) {
            return Err(ModelError::new(
                "model_file_hash_mismatch",
                format!("model file hash mismatch: {}", file.path),
            ));
        }
    }
    let required_paths = std::iter::once(&manifest.result_schema.path)
        .chain(manifest.variants.iter().map(|variant| &variant.artifact))
        .chain(
            manifest
                .self_test
                .iter()
                .flat_map(|case| [&case.input, &case.expected]),
        );
    for required in required_paths {
        let required = safe_relative_path(required)?;
        if !paths.contains(&required) {
            return Err(ModelError::new(
                "invalid_model_manifest",
                format!("required file is not hash-declared: {}", required.display()),
            ));
        }
    }
    let schema_bytes = std::fs::read(root.join(safe_relative_path(&manifest.result_schema.path)?))
        .map_err(|error| ModelError::io("read result schema", error))?;
    base::serde_json::from_slice::<base::serde_json::Value>(&schema_bytes).map_err(|error| {
        ModelError::new(
            "invalid_result_schema",
            format!("model result schema is invalid JSON: {error}"),
        )
    })?;

    let compatible_variants = manifest
        .variants
        .iter()
        .filter(|variant| {
            variant.architecture == policy.architecture
                && policy.available_runtimes.contains(&variant.runtime)
                && (variant.accelerator.is_empty()
                    || policy.available_accelerators.contains(&variant.accelerator))
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_variant = match compatible_variants.as_slice() {
        [selected] => selected.clone(),
        [] => {
            return Err(ModelError::new(
                "model_runtime_incompatible",
                "no model variant matches this architecture and available runtime",
            ));
        }
        _ => {
            return Err(ModelError::new(
                "model_variant_ambiguous",
                "multiple model variants match this architecture and available runtime",
            ));
        }
    };
    Ok(VerifiedModelPackage {
        root: root.to_path_buf(),
        manifest,
        selected_variant,
        manifest_sha256: format!("{:x}", Sha256::digest(manifest_bytes)),
        policy: policy.clone(),
    })
}

fn validate_manifest(manifest: &ModelPackageManifest, policy: &PackagePolicy) -> ModelResult<()> {
    if manifest.api_version != API_VERSION || manifest.kind != PACKAGE_KIND {
        return Err(ModelError::new(
            "unsupported_model_schema",
            "unsupported model package api_version or kind",
        ));
    }
    for value in [
        &manifest.metadata.model_id,
        &manifest.metadata.version,
        &manifest.metadata.revision,
    ] {
        validate_identifier(value)?;
    }
    if manifest.capabilities.is_empty()
        || manifest
            .capabilities
            .iter()
            .any(|value| value.trim().is_empty())
        || manifest.variants.is_empty()
        || manifest.files.is_empty()
    {
        return Err(ModelError::new(
            "invalid_model_manifest",
            "capabilities, variants and files must not be empty",
        ));
    }
    if !policy.allowed_result_schemas.contains(&(
        manifest.result_schema.name.clone(),
        manifest.result_schema.version,
    )) {
        return Err(ModelError::new(
            "unsupported_result_schema",
            "model result schema is not allowed",
        ));
    }
    if !policy.approved_spdx.contains(&manifest.license.spdx)
        || (!manifest.license.license_ref.is_empty()
            && !policy
                .available_license_refs
                .contains(&manifest.license.license_ref))
    {
        return Err(ModelError::new(
            "model_license_unavailable",
            "model license is unavailable or not approved",
        ));
    }
    if !policy
        .trusted_signing_keys
        .contains_key(&manifest.signing.key_id)
        || manifest.signing.signature.trim().is_empty()
    {
        return Err(ModelError::new(
            "model_signature_untrusted",
            "model package signing metadata is missing or untrusted",
        ));
    }
    if manifest.resources.memory_mb > policy.max_memory_mb
        || manifest.resources.vram_mb > policy.max_vram_mb
        || manifest.resources.max_batch == 0
    {
        return Err(ModelError::new(
            "model_resource_limit_exceeded",
            "model resource hints exceed configured limits",
        ));
    }
    if let Some(execution) = &manifest.execution {
        validate_execution_contract(
            execution,
            &policy.execution_limits,
            manifest.resources.memory_mb,
        )?;
    }
    for case in &manifest.self_test {
        if let Some(oracle) = &case.oracle
            && (oracle.kind != "json_numeric_v1"
                || !oracle.abs_tolerance.is_finite()
                || oracle.abs_tolerance < 0.0
                || !oracle.rel_tolerance.is_finite()
                || oracle.rel_tolerance < 0.0)
        {
            return Err(ModelError::new(
                "invalid_model_execution_contract",
                "self-test oracle is unsupported or invalid",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_execution_contract(
    execution: &ExecutionContract,
    limits: &ExecutionLimits,
    resource_memory_mb: u64,
) -> ModelResult<()> {
    let input = &execution.input;
    if execution.version != 1
        || input.kind != "encoded_image_tensor_v1"
        || input.accepted_media_types.is_empty()
        || input.max_bytes == 0
        || input.max_width == 0
        || input.max_height == 0
        || input.tensor.name.trim().is_empty()
        || input.tensor.dtype != "f32"
        || input.tensor.layout != "nchw"
        || input.tensor.shape.len() != 4
        || input.tensor.shape[0] != 1
        || input.tensor.shape[1] != 3
        || input.tensor.shape[2] == 0
        || input.tensor.shape[3] == 0
        || input.preprocess.resize != "exact"
        || input.preprocess.interpolation != "bilinear"
        || input.preprocess.color != "rgb"
        || !input.preprocess.scale.is_finite()
        || input.preprocess.mean.iter().any(|value| !value.is_finite())
        || input
            .preprocess
            .std
            .iter()
            .any(|value| !value.is_finite() || *value == 0.0)
        || execution.outputs.is_empty()
        || execution.postprocess.kind != "tensor_json_v1"
    {
        return Err(ModelError::new(
            "invalid_model_execution_contract",
            "execution contract v1 contains unsupported or invalid values",
        ));
    }
    let supported_media = ["image/jpeg", "image/png", "image/webp"];
    if input
        .accepted_media_types
        .iter()
        .any(|media| !supported_media.contains(&media.as_str()))
    {
        return Err(ModelError::new(
            "invalid_model_execution_contract",
            "execution contract declares an unsupported encoded image media type",
        ));
    }
    let input_elements = checked_element_count(&input.tensor.shape)?;
    let input_bytes = input_elements
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| {
            ModelError::new(
                "model_execution_resource_limit_exceeded",
                "input tensor byte count overflows this platform",
            )
        })?;
    if input_elements > limits.max_input_elements || input_bytes > limits.max_input_bytes {
        return Err(ModelError::new(
            "model_execution_resource_limit_exceeded",
            "input tensor exceeds configured element or byte limits",
        ));
    }
    if execution.outputs.len() > limits.max_output_tensors {
        return Err(ModelError::new(
            "model_execution_resource_limit_exceeded",
            "output tensor count exceeds the configured limit",
        ));
    }
    let mut output_names = HashSet::new();
    let mut output_elements = 0_usize;
    for output in &execution.outputs {
        if output.name.trim().is_empty()
            || !output_names.insert(&output.name)
            || output.dtype != "f32"
            || !output.layout.is_empty()
        {
            return Err(ModelError::new(
                "invalid_model_execution_contract",
                "execution output contract is unsupported or duplicated",
            ));
        }
        output_elements = output_elements
            .checked_add(checked_element_count(&output.shape)?)
            .ok_or_else(|| {
                ModelError::new(
                    "model_execution_resource_limit_exceeded",
                    "aggregate output element count overflows this platform",
                )
            })?;
    }
    let output_bytes = output_elements
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| {
            ModelError::new(
                "model_execution_resource_limit_exceeded",
                "aggregate output byte count overflows this platform",
            )
        })?;
    let contract_bytes = input_bytes.checked_add(output_bytes).ok_or_else(|| {
        ModelError::new(
            "model_execution_resource_limit_exceeded",
            "aggregate tensor byte count overflows this platform",
        )
    })?;
    let resource_bytes = usize::try_from(resource_memory_mb)
        .ok()
        .and_then(|memory| memory.checked_mul(1024 * 1024))
        .ok_or_else(|| {
            ModelError::new(
                "model_execution_resource_limit_exceeded",
                "declared model memory does not fit this platform",
            )
        })?;
    if output_elements > limits.max_output_elements
        || output_bytes > limits.max_output_bytes
        || contract_bytes > resource_bytes
    {
        return Err(ModelError::new(
            "model_execution_resource_limit_exceeded",
            "output tensors exceed configured or declared resource limits",
        ));
    }
    Ok(())
}

pub(crate) fn checked_element_count(shape: &[u64]) -> ModelResult<usize> {
    if shape.is_empty() || shape.contains(&0) {
        return Err(ModelError::new(
            "invalid_model_execution_contract",
            "tensor shape must contain fixed positive dimensions",
        ));
    }
    shape.iter().try_fold(1_usize, |count, dimension| {
        let dimension = usize::try_from(*dimension).map_err(|_| {
            ModelError::new(
                "invalid_model_execution_contract",
                "tensor dimension does not fit this platform",
            )
        })?;
        count.checked_mul(dimension).ok_or_else(|| {
            ModelError::new(
                "invalid_model_execution_contract",
                "tensor element count overflows this platform",
            )
        })
    })
}

pub fn model_package_signing_payload(manifest: &ModelPackageManifest) -> ModelResult<Vec<u8>> {
    let mut unsigned = manifest.clone();
    unsigned.signing.signature.clear();
    base::serde_json::to_vec(&unsigned)
        .map_err(|error| ModelError::new("invalid_model_manifest", error.to_string()))
}

fn verify_signature(manifest: &ModelPackageManifest, policy: &PackagePolicy) -> ModelResult<()> {
    let public_key = policy
        .trusted_signing_keys
        .get(&manifest.signing.key_id)
        .ok_or_else(|| {
            ModelError::new(
                "model_signature_untrusted",
                "model package signing key is not trusted",
            )
        })?;
    let signature = base::base64::engine::general_purpose::STANDARD
        .decode(&manifest.signing.signature)
        .map_err(|_| {
            ModelError::new(
                "model_signature_invalid",
                "model package signature is not valid base64",
            )
        })?;
    let payload = model_package_signing_payload(manifest)?;
    base::artifact::verify_ed25519(&payload, public_key, &signature).map_err(|_| {
        ModelError::new(
            "model_signature_invalid",
            "model package signature verification failed",
        )
    })
}

fn validate_identifier(value: &str) -> ModelResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(ModelError::new(
            "invalid_model_manifest",
            "model identity contains an unsafe value",
        ));
    }
    Ok(())
}

pub(crate) fn safe_relative_path(value: &str) -> ModelResult<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ModelError::new(
            "model_path_invalid",
            format!("model package path is not confined: {value}"),
        ));
    }
    Ok(path.to_path_buf())
}

fn regular_file_metadata(path: &Path) -> ModelResult<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ModelError::io("inspect model package file", error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ModelError::new(
            "invalid_model_package",
            format!("package entry is not a regular file: {}", path.display()),
        ));
    }
    Ok(metadata)
}

fn confined_regular_file_metadata(root: &Path, relative: &Path) -> ModelResult<std::fs::Metadata> {
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        current.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| ModelError::io("inspect model package path", error))?;
        if metadata.file_type().is_symlink()
            || (index + 1 < component_count && !metadata.is_dir())
            || (index + 1 == component_count && !metadata.is_file())
        {
            return Err(ModelError::new(
                "invalid_model_package",
                format!(
                    "package path is not a confined regular file: {}",
                    current.display()
                ),
            ));
        }
    }
    regular_file_metadata(&current)
}
