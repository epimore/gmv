use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek},
    path::{Component, Path, PathBuf},
};

use base::{
    base64::Engine,
    serde::{Deserialize, Serialize},
    sha2::{Digest, Sha256},
};

pub const MODEL_PACKAGE_API_VERSION: &str = "gmv.ai/v1";
pub const MODEL_PACKAGE_KIND: &str = "ModelPlugin";
pub const MODEL_PACKAGE_MANIFEST_PATH: &str = "manifest.yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPackageError {
    pub code: &'static str,
    pub message: String,
}

impl ModelPackageError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ModelPackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ModelPackageError {}

pub type ModelPackageResult<T> = Result<T, ModelPackageError>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct ModelIdentity {
    pub model_id: String,
    pub version: String,
    pub revision: String,
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

#[derive(Debug, Clone, Copy)]
pub struct BundleLimits {
    pub max_manifest_bytes: u64,
    pub max_file_count: usize,
    pub max_package_bytes: u64,
    pub max_path_bytes: usize,
    pub max_path_depth: usize,
    pub max_result_schema_bytes: u64,
}

impl Default for BundleLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 256 * 1024,
            max_file_count: 256,
            max_package_bytes: 4 * 1024 * 1024 * 1024,
            max_path_bytes: 512,
            max_path_depth: 32,
            max_result_schema_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedModelBundle {
    pub manifest: ModelPackageManifest,
    pub manifest_sha256: String,
    pub result_schema: base::serde_json::Value,
}

pub fn model_package_signing_payload(
    manifest: &ModelPackageManifest,
) -> ModelPackageResult<Vec<u8>> {
    let mut unsigned = manifest.clone();
    unsigned.signing.signature.clear();
    base::serde_json::to_vec(&unsigned)
        .map_err(|error| ModelPackageError::new("invalid_model_manifest", error.to_string()))
}

pub fn verify_model_package_signature(
    manifest: &ModelPackageManifest,
    public_key: &[u8],
) -> ModelPackageResult<()> {
    let signature = base::base64::engine::general_purpose::STANDARD
        .decode(&manifest.signing.signature)
        .map_err(|_| {
            ModelPackageError::new(
                "model_signature_invalid",
                "model package signature is not valid base64",
            )
        })?;
    let payload = model_package_signing_payload(manifest)?;
    base::artifact::verify_ed25519(&payload, public_key, &signature).map_err(|_| {
        ModelPackageError::new(
            "model_signature_invalid",
            "model package signature verification failed",
        )
    })
}

pub fn validate_model_package_manifest(manifest: &ModelPackageManifest) -> ModelPackageResult<()> {
    if manifest.api_version != MODEL_PACKAGE_API_VERSION || manifest.kind != MODEL_PACKAGE_KIND {
        return Err(ModelPackageError::new(
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
        || manifest.result_schema.name.trim().is_empty()
        || manifest.result_schema.version == 0
        || manifest.signing.key_id.trim().is_empty()
        || manifest.signing.signature.trim().is_empty()
        || manifest.license.spdx.trim().is_empty()
        || manifest.license.redistribution.trim().is_empty()
        || manifest.resources.max_batch == 0
    {
        return Err(ModelPackageError::new(
            "invalid_model_manifest",
            "model package manifest contains empty or invalid required values",
        ));
    }
    let mut capabilities = HashSet::new();
    if manifest
        .capabilities
        .iter()
        .any(|value| !capabilities.insert(value))
    {
        return Err(ModelPackageError::new(
            "invalid_model_manifest",
            "model package capabilities contain duplicates",
        ));
    }
    if manifest.files.len() > BundleLimits::default().max_file_count {
        return Err(ModelPackageError::new(
            "model_package_too_large",
            "manifest declares too many files",
        ));
    }
    let mut files = HashSet::new();
    for file in &manifest.files {
        let path = safe_relative_path(&file.path, &BundleLimits::default())?;
        let hash = file.sha256.trim_start_matches("sha256:");
        if !files.insert(path)
            || hash.len() != 64
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ModelPackageError::new(
                "invalid_model_manifest",
                "model package file declaration is duplicated or invalid",
            ));
        }
    }
    for variant in &manifest.variants {
        if variant.runtime.trim().is_empty() || variant.architecture.trim().is_empty() {
            return Err(ModelPackageError::new(
                "invalid_model_manifest",
                "model package variant contains an invalid selector",
            ));
        }
        safe_relative_path(&variant.artifact, &BundleLimits::default())?;
    }
    safe_relative_path(&manifest.result_schema.path, &BundleLimits::default())?;
    for case in &manifest.self_test {
        safe_relative_path(&case.input, &BundleLimits::default())?;
        safe_relative_path(&case.expected, &BundleLimits::default())?;
        if let Some(oracle) = &case.oracle
            && (oracle.kind != "json_numeric_v1"
                || !oracle.abs_tolerance.is_finite()
                || oracle.abs_tolerance < 0.0
                || !oracle.rel_tolerance.is_finite()
                || oracle.rel_tolerance < 0.0)
        {
            return Err(ModelPackageError::new(
                "invalid_model_execution_contract",
                "self-test oracle is unsupported or invalid",
            ));
        }
    }
    Ok(())
}

pub fn safe_relative_path(value: &str, limits: &BundleLimits) -> ModelPackageResult<PathBuf> {
    let path = Path::new(value);
    let normalized = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => segment.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if value.len() > limits.max_path_bytes
        || value.contains('\\')
        || path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().count() > limits.max_path_depth
        || normalized != value
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ModelPackageError::new(
            "model_path_invalid",
            format!("model package path is not confined: {value}"),
        ));
    }
    Ok(path.to_path_buf())
}

pub fn verify_model_bundle<R: Read + Seek>(
    reader: R,
    trusted_signing_keys: &HashMap<String, Vec<u8>>,
    limits: BundleLimits,
) -> ModelPackageResult<VerifiedModelBundle> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(bundle_io)?;
    let mut paths = HashSet::new();
    let mut manifest = None;
    let mut manifest_sha256 = None;
    let mut declared = HashMap::<PathBuf, ModelFile>::new();
    let mut seen_files = HashSet::new();
    let mut result_schema = None;
    let mut total_bytes = 0_u64;
    let mut entry_count = 0_usize;

    for item in entries {
        entry_count = entry_count.checked_add(1).ok_or_else(|| {
            ModelPackageError::new("model_package_too_large", "bundle entry count overflow")
        })?;
        if entry_count > limits.max_file_count.saturating_mul(2).saturating_add(1) {
            return Err(ModelPackageError::new(
                "model_package_too_large",
                "bundle contains too many entries",
            ));
        }
        let mut entry = item.map_err(bundle_io)?;
        let raw_path = entry.path_bytes();
        let path_text = std::str::from_utf8(raw_path.as_ref())
            .map_err(|_| {
                ModelPackageError::new("model_path_invalid", "bundle path is not valid UTF-8")
            })?
            .to_owned();
        let path = safe_relative_path(&path_text, &limits)?;
        if !paths.insert(path.clone()) {
            return Err(ModelPackageError::new(
                "invalid_model_package",
                format!("duplicate bundle entry: {path_text}"),
            ));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            return Err(ModelPackageError::new(
                "invalid_model_package",
                format!("bundle entry is not a regular file: {path_text}"),
            ));
        }
        let size = entry.size();
        total_bytes = total_bytes.checked_add(size).ok_or_else(|| {
            ModelPackageError::new("model_package_too_large", "bundle size overflow")
        })?;
        if total_bytes > limits.max_package_bytes {
            return Err(ModelPackageError::new(
                "model_package_too_large",
                "bundle exceeds configured byte limit",
            ));
        }

        if manifest.is_none() {
            if path_text != MODEL_PACKAGE_MANIFEST_PATH {
                return Err(ModelPackageError::new(
                    "invalid_model_package",
                    "manifest.yaml must be the first regular-file entry",
                ));
            }
            if size > limits.max_manifest_bytes {
                return Err(ModelPackageError::new(
                    "model_package_too_large",
                    "manifest exceeds configured size limit",
                ));
            }
            let mut bytes = Vec::with_capacity(size as usize);
            entry.read_to_end(&mut bytes).map_err(bundle_io)?;
            let parsed: ModelPackageManifest =
                base::serde_yaml::from_slice(&bytes).map_err(|error| {
                    ModelPackageError::new("invalid_model_manifest", error.to_string())
                })?;
            validate_model_package_manifest(&parsed)?;
            if parsed.files.len() > limits.max_file_count {
                return Err(ModelPackageError::new(
                    "model_package_too_large",
                    "manifest declares too many files",
                ));
            }
            let key = trusted_signing_keys
                .get(&parsed.signing.key_id)
                .ok_or_else(|| {
                    ModelPackageError::new(
                        "model_signature_untrusted",
                        "model package signing key is not trusted",
                    )
                })?;
            verify_model_package_signature(&parsed, key)?;
            for file in &parsed.files {
                let file_path = safe_relative_path(&file.path, &limits)?;
                if declared.insert(file_path, file.clone()).is_some() {
                    return Err(ModelPackageError::new(
                        "invalid_model_manifest",
                        format!("duplicate file declaration: {}", file.path),
                    ));
                }
            }
            manifest_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
            manifest = Some(parsed);
            continue;
        }

        let expected = declared.get(&path).ok_or_else(|| {
            ModelPackageError::new(
                "invalid_model_package",
                format!("bundle contains undeclared file: {path_text}"),
            )
        })?;
        if size != expected.size {
            return Err(ModelPackageError::new(
                "model_file_size_mismatch",
                format!("model file size mismatch: {path_text}"),
            ));
        }
        let is_schema = manifest
            .as_ref()
            .is_some_and(|value| value.result_schema.path == path_text);
        if is_schema && size > limits.max_result_schema_bytes {
            return Err(ModelPackageError::new(
                "model_package_too_large",
                "result schema exceeds configured size limit",
            ));
        }
        let mut hasher = Sha256::new();
        let mut schema_bytes = if is_schema {
            Some(Vec::with_capacity(size as usize))
        } else {
            None
        };
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer).map_err(bundle_io)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            if let Some(bytes) = &mut schema_bytes {
                bytes.extend_from_slice(&buffer[..read]);
            }
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected.sha256.trim_start_matches("sha256:")) {
            return Err(ModelPackageError::new(
                "model_file_hash_mismatch",
                format!("model file hash mismatch: {path_text}"),
            ));
        }
        seen_files.insert(path.clone());
        if let Some(bytes) = schema_bytes {
            result_schema = Some(base::serde_json::from_slice(&bytes).map_err(|error| {
                ModelPackageError::new(
                    "invalid_result_schema",
                    format!("model result schema is invalid JSON: {error}"),
                )
            })?);
        }
    }

    let manifest = manifest.ok_or_else(|| {
        ModelPackageError::new("invalid_model_package", "bundle is missing manifest.yaml")
    })?;
    if declared.keys().any(|path| !seen_files.contains(path)) {
        return Err(ModelPackageError::new(
            "invalid_model_package",
            "bundle is missing a declared file",
        ));
    }
    let required = std::iter::once(&manifest.result_schema.path)
        .chain(manifest.variants.iter().map(|variant| &variant.artifact))
        .chain(
            manifest
                .self_test
                .iter()
                .flat_map(|case| [&case.input, &case.expected]),
        );
    for value in required {
        let path = safe_relative_path(value, &limits)?;
        if !declared.contains_key(&path) {
            return Err(ModelPackageError::new(
                "invalid_model_manifest",
                format!("required file is not hash-declared: {value}"),
            ));
        }
    }
    Ok(VerifiedModelBundle {
        manifest,
        manifest_sha256: manifest_sha256.expect("manifest hash is set with manifest"),
        result_schema: result_schema.ok_or_else(|| {
            ModelPackageError::new("invalid_result_schema", "bundle is missing result schema")
        })?,
    })
}

fn validate_identifier(value: &str) -> ModelPackageResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(ModelPackageError::new(
            "invalid_model_manifest",
            "model identity contains an unsafe value",
        ));
    }
    Ok(())
}

fn bundle_io(error: impl std::fmt::Display) -> ModelPackageError {
    ModelPackageError::new("invalid_model_package", error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, io::Cursor};

    use base::{
        base64::Engine,
        sha2::{Digest, Sha256},
    };
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    const TEST_KEY: [u8; 32] = [7; 32];

    fn manifest() -> ModelPackageManifest {
        let schema = br#"{"type":"object"}"#;
        let model = b"model";
        let input = b"input";
        let expected = b"expected";
        let mut value = ModelPackageManifest {
            api_version: MODEL_PACKAGE_API_VERSION.to_string(),
            kind: MODEL_PACKAGE_KIND.to_string(),
            metadata: ModelIdentity {
                model_id: "detector".to_string(),
                version: "1.2.3".to_string(),
                revision: "r1".to_string(),
            },
            capabilities: vec!["object-detection".to_string()],
            result_schema: ResultSchema {
                name: "detections".to_string(),
                version: 1,
                path: "schemas/result.json".to_string(),
            },
            variants: vec![RuntimeVariant {
                runtime: "onnx-cpu".to_string(),
                runtime_contract_version: 1,
                architecture: "x86_64".to_string(),
                accelerator: "cpu".to_string(),
                artifact: "models/model.onnx".to_string(),
            }],
            execution: None,
            resources: ResourceHints {
                memory_mb: 256,
                vram_mb: 0,
                max_batch: 1,
            },
            license: LicenseSpec {
                spdx: "Apache-2.0".to_string(),
                commercial_use: true,
                redistribution: "allowed".to_string(),
                license_ref: String::new(),
            },
            self_test: vec![SelfTestCase {
                input: "tests/input.bin".to_string(),
                expected: "tests/expected.json".to_string(),
                oracle: Some(SelfTestOracle {
                    kind: "json_numeric_v1".to_string(),
                    abs_tolerance: 0.001,
                    rel_tolerance: 0.001,
                }),
            }],
            files: vec![
                model_file("schemas/result.json", schema),
                model_file("models/model.onnx", model),
                model_file("tests/input.bin", input),
                model_file("tests/expected.json", expected),
            ],
            signing: SigningSpec {
                key_id: "test-key".to_string(),
                signature: String::new(),
            },
        };
        let signature =
            SigningKey::from_bytes(&TEST_KEY).sign(&model_package_signing_payload(&value).unwrap());
        value.signing.signature =
            base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        value
    }

    fn model_file(path: &str, bytes: &[u8]) -> ModelFile {
        ModelFile {
            path: path.to_string(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }

    fn trusted_keys() -> HashMap<String, Vec<u8>> {
        HashMap::from([(
            "test-key".to_string(),
            SigningKey::from_bytes(&TEST_KEY)
                .verifying_key()
                .to_bytes()
                .to_vec(),
        )])
    }

    fn append(builder: &mut tar::Builder<Vec<u8>>, path: &str, bytes: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, bytes).unwrap();
    }

    fn bundle(value: &ModelPackageManifest, undeclared: bool) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        append(
            &mut builder,
            MODEL_PACKAGE_MANIFEST_PATH,
            base::serde_yaml::to_string(value).unwrap().as_bytes(),
        );
        append(&mut builder, "schemas/result.json", br#"{"type":"object"}"#);
        append(&mut builder, "models/model.onnx", b"model");
        append(&mut builder, "tests/input.bin", b"input");
        append(&mut builder, "tests/expected.json", b"expected");
        if undeclared {
            append(&mut builder, "surprise.txt", b"no");
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn signing_payload_is_canonical_and_stable() {
        let value = manifest();
        let payload = model_package_signing_payload(&value).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(payload)),
            "53a4c30343c3800931550b5f23db68dc31791f67e2871e6aa1132ec696113306"
        );
        assert_eq!(
            value.signing.signature,
            "NQW74DIg7RaPyGdNxVN3sW1c/EgTJG9gKxjB5mIpiKittFvNli3zb9idzOYqLs49x0qVGZSTYkAzLwIdohsOBw=="
        );
        verify_model_package_signature(&value, &trusted_keys()["test-key"]).unwrap();
    }

    #[test]
    fn verifies_streamed_bundle_without_extracting() {
        let value = manifest();
        let verified = verify_model_bundle(
            Cursor::new(bundle(&value, false)),
            &trusted_keys(),
            BundleLimits::default(),
        )
        .unwrap();
        assert_eq!(verified.manifest.metadata, value.metadata);
        assert_eq!(verified.result_schema["type"], "object");
    }

    #[test]
    fn rejects_untrusted_tampered_and_undeclared_content() {
        let value = manifest();
        assert_eq!(
            verify_model_bundle(
                Cursor::new(bundle(&value, false)),
                &HashMap::new(),
                BundleLimits::default(),
            )
            .unwrap_err()
            .code,
            "model_signature_untrusted"
        );
        let mut tampered = value.clone();
        tampered.metadata.revision = "r2".to_string();
        assert_eq!(
            verify_model_bundle(
                Cursor::new(bundle(&tampered, false)),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err()
            .code,
            "model_signature_invalid"
        );
        assert_eq!(
            verify_model_bundle(
                Cursor::new(bundle(&value, true)),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err()
            .code,
            "invalid_model_package"
        );
    }

    #[test]
    fn rejects_non_regular_entries_and_manifest_not_first() {
        let value = manifest();
        let mut builder = tar::Builder::new(Vec::new());
        append(&mut builder, "models/model.onnx", b"model");
        append(
            &mut builder,
            MODEL_PACKAGE_MANIFEST_PATH,
            base::serde_yaml::to_string(&value).unwrap().as_bytes(),
        );
        let error = verify_model_bundle(
            Cursor::new(builder.into_inner().unwrap()),
            &trusted_keys(),
            BundleLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_model_package");

        for entry_type in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Char,
            tar::EntryType::Block,
            tar::EntryType::Fifo,
        ] {
            let mut builder = tar::Builder::new(Vec::new());
            append(
                &mut builder,
                MODEL_PACKAGE_MANIFEST_PATH,
                base::serde_yaml::to_string(&value).unwrap().as_bytes(),
            );
            let mut header = tar::Header::new_gnu();
            header.set_path("models/non-regular").unwrap();
            header.set_entry_type(entry_type);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            let error = verify_model_bundle(
                Cursor::new(builder.into_inner().unwrap()),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err();
            assert_eq!(error.code, "invalid_model_package");
        }
    }

    #[test]
    fn rejects_invalid_paths_and_configured_bounds() {
        let limits = BundleLimits::default();
        assert_eq!(
            safe_relative_path("../escape", &limits).unwrap_err().code,
            "model_path_invalid"
        );
        assert_eq!(
            safe_relative_path("/absolute", &limits).unwrap_err().code,
            "model_path_invalid"
        );
        assert!(safe_relative_path("a//b", &limits).is_err());
        assert!(safe_relative_path("a\\b", &limits).is_err());

        let value = manifest();
        let tiny = BundleLimits {
            max_package_bytes: 8,
            ..BundleLimits::default()
        };
        assert_eq!(
            verify_model_bundle(Cursor::new(bundle(&value, false)), &trusted_keys(), tiny)
                .unwrap_err()
                .code,
            "model_package_too_large"
        );
    }

    #[test]
    fn rejects_duplicate_malformed_oversized_and_integrity_mismatch_bundles() {
        let value = manifest();
        let mut duplicate = tar::Builder::new(Vec::new());
        append(
            &mut duplicate,
            MODEL_PACKAGE_MANIFEST_PATH,
            base::serde_yaml::to_string(&value).unwrap().as_bytes(),
        );
        append(
            &mut duplicate,
            "schemas/result.json",
            br#"{"type":"object"}"#,
        );
        append(
            &mut duplicate,
            "schemas/result.json",
            br#"{"type":"object"}"#,
        );
        assert_eq!(
            verify_model_bundle(
                Cursor::new(duplicate.into_inner().unwrap()),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err()
            .code,
            "invalid_model_package"
        );
        assert!(
            verify_model_bundle(
                Cursor::new(vec![1_u8; 512]),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .is_err()
        );

        let manifest_limit = BundleLimits {
            max_manifest_bytes: 1,
            ..BundleLimits::default()
        };
        assert_eq!(
            verify_model_bundle(
                Cursor::new(bundle(&value, false)),
                &trusted_keys(),
                manifest_limit,
            )
            .unwrap_err()
            .code,
            "model_package_too_large"
        );

        let mut too_many = value.clone();
        for index in 0..=256 {
            too_many
                .files
                .push(model_file(&format!("extra/{index}"), b"x"));
        }
        too_many.signing.signature.clear();
        let signature = SigningKey::from_bytes(&TEST_KEY)
            .sign(&model_package_signing_payload(&too_many).unwrap());
        too_many.signing.signature =
            base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        assert_eq!(
            verify_model_bundle(
                Cursor::new(bundle(&too_many, false)),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err()
            .code,
            "model_package_too_large"
        );

        for mutate in ["size", "hash"] {
            let mut changed = value.clone();
            if mutate == "size" {
                changed.files[0].size += 1;
            } else {
                changed.files[0].sha256 = "0".repeat(64);
            }
            changed.signing.signature.clear();
            let signature = SigningKey::from_bytes(&TEST_KEY)
                .sign(&model_package_signing_payload(&changed).unwrap());
            changed.signing.signature =
                base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
            let error = verify_model_bundle(
                Cursor::new(bundle(&changed, false)),
                &trusted_keys(),
                BundleLimits::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error.code,
                "model_file_size_mismatch" | "model_file_hash_mismatch"
            ));
        }
    }
}
