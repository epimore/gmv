use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use base::{
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    tokio_util::sync::CancellationToken,
};
use gmv_protocol::{
    avai::model_management::v1::ModelIdentity as RpcModelIdentity,
    common::v1::{ModelDeliveryCorrelation, ModelVariantSelector, OperationRef},
};

use crate::model::{ModelError, ModelIdentity, ModelResult, RuntimeCallContext};

use super::now_epoch_ms;

const MAX_ID_BYTES: usize = 128;
pub(super) const MAX_DEADLINE_AHEAD_MS: i64 = 2 * 60 * 60 * 1_000;

pub(super) fn runtime_context_from_epoch(
    deadline_epoch_ms: i64,
    cancellation: CancellationToken,
) -> RuntimeCallContext {
    let remaining = deadline_epoch_ms.saturating_sub(now_epoch_ms()).max(0);
    RuntimeCallContext {
        deadline: Instant::now()
            + Duration::from_millis(u64::try_from(remaining).unwrap_or_default()),
        cancellation,
    }
}

pub(super) fn validate_operation_identity(
    operation: Option<OperationRef>,
) -> ModelResult<OperationRef> {
    let operation = operation.ok_or_else(|| {
        ModelError::new("model_operation_invalid", "operation identity is required")
    })?;
    if !valid_bounded_token(&operation.operation_id)
        || !valid_bounded_token(&operation.idempotency_key)
    {
        return Err(ModelError::new(
            "model_operation_invalid",
            "operation identifiers are invalid",
        ));
    }
    Ok(operation)
}

pub(super) fn validate_new_deadline(deadline: i64, now: i64) -> ModelResult<()> {
    if deadline <= now || deadline > now.saturating_add(MAX_DEADLINE_AHEAD_MS) {
        return Err(ModelError::new(
            "model_deadline_invalid",
            "operation deadline is outside the allowed window",
        ));
    }
    Ok(())
}

pub(super) fn rpc_identity(identity: Option<RpcModelIdentity>) -> ModelResult<ModelIdentity> {
    let identity = identity
        .ok_or_else(|| ModelError::new("model_identity_invalid", "model identity is required"))?;
    if [&identity.model_id, &identity.version, &identity.revision]
        .iter()
        .any(|part| !valid_model_identifier(part))
    {
        return Err(ModelError::new(
            "model_identity_invalid",
            "model identity is invalid",
        ));
    }
    Ok(ModelIdentity {
        model_id: identity.model_id,
        version: identity.version,
        revision: identity.revision,
    })
}

pub(super) fn validate_correlation(
    correlation: Option<ModelDeliveryCorrelation>,
) -> ModelResult<ModelDeliveryCorrelation> {
    let correlation = correlation.ok_or_else(|| {
        ModelError::new(
            "model_delivery_correlation_invalid",
            "model delivery correlation is required",
        )
    })?;
    if [
        &correlation.deployment_id,
        &correlation.installation_id,
        &correlation.host_id,
        &correlation.component_id,
        &correlation.assignment_id,
    ]
    .iter()
    .any(|value| !valid_model_identifier(value))
    {
        return Err(ModelError::new(
            "model_delivery_correlation_invalid",
            "model delivery correlation contains an invalid identifier",
        ));
    }
    Ok(correlation)
}

pub(super) fn validate_selector(
    selector: Option<ModelVariantSelector>,
) -> ModelResult<ModelVariantSelector> {
    let selector = selector.ok_or_else(|| {
        ModelError::new("model_selector_missing", "exact model selector is required")
    })?;
    if !valid_model_identifier(&selector.runtime)
        || selector.runtime_contract_version == 0
        || !valid_model_identifier(&selector.architecture)
        || (!selector.accelerator.is_empty() && !valid_model_identifier(&selector.accelerator))
    {
        return Err(ModelError::new(
            "model_selector_invalid",
            "exact model selector is invalid",
        ));
    }
    Ok(selector)
}

fn valid_model_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_bounded_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn valid_stage_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn ensure_before_deadline(deadline: i64) -> ModelResult<()> {
    if now_epoch_ms() >= deadline {
        Err(ModelError::new(
            "model_deadline_exceeded",
            "model operation deadline expired",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn runtime_unavailable() -> ModelError {
    ModelError::new(
        "model_runtime_unavailable",
        "runtime provider is unavailable",
    )
}

#[cfg(unix)]
pub(super) fn trusted_stage_candidate(root: &Path, stage_id: &str) -> ModelResult<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root_meta = std::fs::symlink_metadata(root)
        .map_err(|error| ModelError::io("inspect trusted import root", error))?;
    if !root_meta.is_dir()
        || root_meta.file_type().is_symlink()
        || root_meta.permissions().mode() & 0o022 != 0
    {
        return Err(ModelError::new(
            "model_stage_insecure",
            "trusted import root must be a non-writable real directory",
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ModelError::io("resolve trusted import root", error))?;
    let candidate = root.join(stage_id);
    let candidate_meta = std::fs::symlink_metadata(&candidate)
        .map_err(|error| ModelError::io("inspect staged model", error))?;
    if !candidate_meta.is_dir()
        || candidate_meta.file_type().is_symlink()
        || candidate_meta.permissions().mode() & 0o022 != 0
        || candidate_meta.uid() != root_meta.uid()
    {
        return Err(ModelError::new(
            "model_stage_insecure",
            "staged model must be an owner-matched non-writable real directory",
        ));
    }
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| ModelError::io("resolve staged model", error))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(ModelError::new(
            "model_stage_invalid",
            "staged model resolves outside the trusted root",
        ));
    }
    Ok(candidate)
}

#[cfg(not(unix))]
pub(super) fn trusted_stage_candidate(_root: &Path, _stage_id: &str) -> ModelResult<PathBuf> {
    Err(ModelError::new(
        "model_stage_unsupported",
        "local model staging requires Unix filesystem security",
    ))
}

pub(super) fn encode_page_token(identity: &ModelIdentity) -> String {
    URL_SAFE_NO_PAD.encode(format!(
        "{}\0{}\0{}",
        identity.model_id, identity.version, identity.revision
    ))
}

pub(super) fn decode_page_token(token: &str) -> ModelResult<Option<ModelIdentity>> {
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() > 1024 {
        return Err(ModelError::new(
            "model_page_token_invalid",
            "page token is invalid",
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| ModelError::new("model_page_token_invalid", "page token is invalid"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ModelError::new("model_page_token_invalid", "page token is invalid"))?;
    let parts = text.split('\0').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| !valid_bounded_token(part)) {
        return Err(ModelError::new(
            "model_page_token_invalid",
            "page token is invalid",
        ));
    }
    Ok(Some(ModelIdentity {
        model_id: parts[0].to_string(),
        version: parts[1].to_string(),
        revision: parts[2].to_string(),
    }))
}
