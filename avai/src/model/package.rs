use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use base::{
    base64::Engine,
    serde::{Deserialize, Serialize},
    sha2::{Digest, Sha256},
};

use super::{ModelError, ModelResult};

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
    pub architecture: String,
    #[serde(default)]
    pub accelerator: String,
    pub artifact: String,
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

    let selected_variant = manifest
        .variants
        .iter()
        .find(|variant| {
            variant.architecture == policy.architecture
                && policy.available_runtimes.contains(&variant.runtime)
                && (variant.accelerator.is_empty()
                    || policy.available_accelerators.contains(&variant.accelerator))
        })
        .cloned()
        .ok_or_else(|| {
            ModelError::new(
                "model_runtime_incompatible",
                "no model variant matches this architecture and available runtime",
            )
        })?;
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
    Ok(())
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
