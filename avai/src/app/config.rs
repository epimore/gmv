use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use avai::model::{ExecutionLimits, PackagePolicy};
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::GlobalResult;
use base::serde::Deserialize;

use super::{external_error, global_error};

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard", check)]
pub(super) struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    pub(super) endpoint: String,
}

impl CheckFromConf for GuardConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.endpoint.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "guard.endpoint must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server", check)]
pub(super) struct ServerConf {
    pub(super) installation_id: String,
    pub(super) host_id: String,
    #[serde(default = "default_node_id")]
    pub(super) node_id: String,
    #[serde(default = "default_host")]
    pub(super) host: String,
    #[serde(default = "default_grpc_port")]
    pub(super) grpc_port: u16,
    #[serde(default = "default_capabilities")]
    pub(super) capabilities: Vec<String>,
    #[serde(default = "default_task_database_path")]
    pub(super) task_database_path: String,
    #[serde(default = "default_object_root")]
    pub(super) object_root: String,
    #[serde(default = "default_uds_socket_root")]
    pub(super) uds_socket_root: String,
    #[serde(default = "default_upload_listen_addr")]
    pub(super) upload_listen_addr: SocketAddr,
    #[serde(default = "default_upload_public_url")]
    pub(super) upload_public_url: String,
    #[serde(default = "default_max_image_bytes")]
    pub(super) max_image_bytes: usize,
    #[serde(default = "default_max_result_bytes")]
    pub(super) max_result_bytes: usize,
    #[serde(default = "default_task_queue_size")]
    pub(super) task_queue_size: usize,
    #[serde(default = "default_task_worker_count")]
    pub(super) task_worker_count: usize,
    #[serde(default)]
    pub(super) allow_private_image_urls: bool,
    #[serde(default)]
    pub(super) allowed_internal_hosts: Vec<String>,
    #[serde(default)]
    pub(super) management_socket: Option<PathBuf>,
    #[serde(default = "default_management_component_id")]
    pub(super) management_component_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
struct ResultSchemaConf {
    name: String,
    version: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "model", check)]
pub(super) struct ModelConf {
    #[serde(default = "default_model_database_path")]
    pub(super) database_path: String,
    #[serde(default = "default_model_root")]
    pub(super) root: String,
    #[serde(default = "default_trusted_import_root")]
    pub(super) trusted_import_root: String,
    #[serde(default = "default_model_architecture")]
    architecture: String,
    #[serde(default)]
    pub(super) allowed_runtime_ids: Vec<String>,
    #[serde(default = "default_model_accelerators")]
    allowed_accelerators: Vec<String>,
    #[serde(default)]
    allowed_result_schemas: Vec<ResultSchemaConf>,
    #[serde(default)]
    approved_spdx: Vec<String>,
    #[serde(default)]
    available_license_refs: Vec<String>,
    #[serde(default)]
    trusted_signing_keys: HashMap<String, String>,
    #[serde(default = "default_max_manifest_bytes")]
    max_manifest_bytes: u64,
    #[serde(default = "default_max_model_files")]
    max_file_count: usize,
    #[serde(default = "default_max_package_bytes")]
    max_package_bytes: u64,
    #[serde(default = "default_max_model_memory_mb")]
    pub(super) max_memory_mb: u64,
    #[serde(default = "default_max_model_vram_mb")]
    pub(super) max_vram_mb: u64,
    #[serde(default = "default_max_loaded_models")]
    pub(super) max_loaded_models: usize,
    #[serde(default = "default_native_worker_count")]
    pub(super) native_worker_count: usize,
    #[serde(default = "default_native_queue_capacity")]
    pub(super) native_queue_capacity: usize,
    #[serde(default = "default_ort_thread_count")]
    pub(super) ort_intra_threads: usize,
    #[serde(default = "default_ort_thread_count")]
    pub(super) ort_inter_threads: usize,
    #[serde(default = "default_native_shutdown_timeout_ms")]
    pub(super) native_shutdown_timeout_ms: u64,
    #[serde(default = "default_max_input_tensor_elements")]
    max_input_tensor_elements: usize,
    #[serde(default = "default_max_input_tensor_bytes")]
    max_input_tensor_bytes: usize,
    #[serde(default = "default_max_output_tensor_count")]
    max_output_tensor_count: usize,
    #[serde(default = "default_max_output_tensor_elements")]
    max_output_tensor_elements: usize,
    #[serde(default = "default_max_output_tensor_bytes")]
    max_output_tensor_bytes: usize,
    #[serde(default = "default_operation_receipt_capacity")]
    pub(super) operation_receipt_capacity: usize,
    #[serde(default = "default_operation_retention_ms")]
    pub(super) operation_retention_ms: i64,
    #[serde(default = "default_mutation_concurrency")]
    pub(super) mutation_concurrency: usize,
}

impl CheckFromConf for ModelConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.database_path.trim().is_empty()
            || self.root.trim().is_empty()
            || self.trusted_import_root.trim().is_empty()
            || self.architecture.trim().is_empty()
        {
            return Err(FieldCheckError::BizError(
                "model paths and architecture must not be empty".to_string(),
            ));
        }
        if self.max_manifest_bytes == 0
            || self.max_file_count == 0
            || self.max_package_bytes == 0
            || self.max_memory_mb == 0
            || self.max_loaded_models == 0
            || self.native_worker_count == 0
            || self.native_worker_count > 64
            || self.native_queue_capacity == 0
            || self.ort_intra_threads == 0
            || self.ort_inter_threads == 0
            || self.native_shutdown_timeout_ms == 0
            || self.native_shutdown_timeout_ms > 60_000
            || self.max_input_tensor_elements == 0
            || self.max_input_tensor_bytes == 0
            || self.max_output_tensor_count == 0
            || self.max_output_tensor_elements == 0
            || self.max_output_tensor_bytes == 0
            || self.operation_receipt_capacity == 0
            || self.operation_receipt_capacity > 4096
            || self.operation_retention_ms < 24 * 60 * 60 * 1_000
            || self.mutation_concurrency != 1
        {
            return Err(FieldCheckError::BizError(
                "model package, runtime and operation bounds are invalid".to_string(),
            ));
        }
        Ok(())
    }
}

impl CheckFromConf for ServerConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.installation_id.trim().is_empty() || self.host_id.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.installation_id and server.host_id must not be empty".to_string(),
            ));
        }
        if self.node_id.trim().is_empty() || self.host.trim().is_empty() || self.grpc_port == 0 {
            return Err(FieldCheckError::BizError(
                "server.node_id, server.host and server.grpc_port are required".to_string(),
            ));
        }
        if self.task_queue_size == 0
            || self.task_worker_count == 0
            || self.max_image_bytes == 0
            || self.max_result_bytes == 0
        {
            return Err(FieldCheckError::BizError(
                "Avai task capacity values must be positive".to_string(),
            ));
        }
        if let Some(socket) = &self.management_socket
            && (!socket.is_absolute()
                || socket.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::CurDir | std::path::Component::ParentDir
                    )
                }))
        {
            return Err(FieldCheckError::BizError(
                "server.management_socket must be an absolute normalized path".to_string(),
            ));
        }
        if self.management_component_id.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.management_component_id must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

impl ModelConf {
    pub(super) fn package_policy(&self) -> GlobalResult<PackagePolicy> {
        use base::base64::Engine;
        let trusted_signing_keys = self
            .trusted_signing_keys
            .iter()
            .map(|(key_id, encoded)| {
                base::base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map(|key| (key_id.clone(), key))
                    .map_err(external_error)
            })
            .collect::<GlobalResult<HashMap<_, _>>>()?;
        Ok(PackagePolicy {
            architecture: self.architecture.clone(),
            available_runtimes: self.allowed_runtime_ids.iter().cloned().collect(),
            available_accelerators: self.allowed_accelerators.iter().cloned().collect(),
            allowed_result_schemas: self
                .allowed_result_schemas
                .iter()
                .map(|schema| (schema.name.clone(), schema.version))
                .collect(),
            approved_spdx: self.approved_spdx.iter().cloned().collect(),
            available_license_refs: self.available_license_refs.iter().cloned().collect(),
            trusted_signing_keys,
            max_manifest_bytes: self.max_manifest_bytes,
            max_file_count: self.max_file_count,
            max_package_bytes: self.max_package_bytes,
            max_memory_mb: self.max_memory_mb,
            max_vram_mb: self.max_vram_mb,
            execution_limits: ExecutionLimits {
                max_input_elements: self.max_input_tensor_elements,
                max_input_bytes: self.max_input_tensor_bytes,
                max_output_tensors: self.max_output_tensor_count,
                max_output_elements: self.max_output_tensor_elements,
                max_output_bytes: self.max_output_tensor_bytes,
            },
        })
    }
}

fn default_guard_endpoint() -> String {
    "http://127.0.0.1:18080".to_string()
}

fn default_node_id() -> String {
    "avai-node-1".to_string()
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_grpc_port() -> u16 {
    19080
}

fn default_capabilities() -> Vec<String> {
    vec!["image.metadata.inspect".to_string()]
}

fn default_task_database_path() -> String {
    "./data/avai.db".to_string()
}

fn default_object_root() -> String {
    "./data/objects".to_string()
}

fn default_uds_socket_root() -> String {
    "./run".to_string()
}

fn default_upload_listen_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 19081))
}

fn default_upload_public_url() -> String {
    "http://127.0.0.1:19081".to_string()
}

fn default_max_image_bytes() -> usize {
    16 * 1024 * 1024
}

fn default_max_result_bytes() -> usize {
    1024 * 1024
}

fn default_task_queue_size() -> usize {
    128
}

fn default_task_worker_count() -> usize {
    2
}

fn default_management_component_id() -> String {
    "avai".to_string()
}

fn default_model_database_path() -> String {
    "./data/avai-model.db".to_string()
}
fn default_model_root() -> String {
    "./data/models".to_string()
}
fn default_trusted_import_root() -> String {
    "./data/model-import".to_string()
}
fn default_model_architecture() -> String {
    std::env::consts::ARCH.to_string()
}
fn default_model_accelerators() -> Vec<String> {
    vec!["cpu".to_string()]
}
fn default_max_manifest_bytes() -> u64 {
    256 * 1024
}
fn default_max_model_files() -> usize {
    256
}
fn default_max_package_bytes() -> u64 {
    4 * 1024 * 1024 * 1024
}
fn default_max_model_memory_mb() -> u64 {
    16 * 1024
}
fn default_max_model_vram_mb() -> u64 {
    16 * 1024
}
fn default_max_loaded_models() -> usize {
    8
}
fn default_native_worker_count() -> usize {
    1
}
fn default_native_queue_capacity() -> usize {
    8
}
fn default_ort_thread_count() -> usize {
    1
}
fn default_native_shutdown_timeout_ms() -> u64 {
    5_000
}
fn default_max_input_tensor_elements() -> usize {
    16 * 1024 * 1024
}
fn default_max_input_tensor_bytes() -> usize {
    64 * 1024 * 1024
}
fn default_max_output_tensor_count() -> usize {
    16
}
fn default_max_output_tensor_elements() -> usize {
    16 * 1024 * 1024
}
fn default_max_output_tensor_bytes() -> usize {
    64 * 1024 * 1024
}
fn default_operation_receipt_capacity() -> usize {
    4096
}
fn default_operation_retention_ms() -> i64 {
    24 * 60 * 60 * 1_000
}
fn default_mutation_concurrency() -> usize {
    1
}

pub(super) fn validate_feedback_root(
    feedback: &Path,
    object: &Path,
    model: &Path,
) -> GlobalResult<PathBuf> {
    if feedback
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(global_error("invalid_feedback_config"));
    }
    std::fs::create_dir_all(object).map_err(external_error)?;
    std::fs::create_dir_all(feedback).map_err(external_error)?;
    if !std::fs::symlink_metadata(feedback)
        .map_err(external_error)?
        .file_type()
        .is_dir()
    {
        return Err(global_error("invalid_feedback_config"));
    }
    let feedback = feedback.canonicalize().map_err(external_error)?;
    for owned in [object, model] {
        let owned = owned.canonicalize().map_err(external_error)?;
        if feedback.starts_with(&owned) || owned.starts_with(&feedback) {
            return Err(global_error("feedback_spool_root_overlap"));
        }
    }
    Ok(feedback)
}
