use std::{
    collections::HashSet,
    ffi::OsStr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use base::{
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    bytes::Bytes,
    cfg_lib::conf,
    cfg_lib::conf::{CheckFromConf, FieldCheckError},
    futures::Stream,
    serde::Deserialize,
    sha2::{Digest, Sha256},
    tokio::{fs, io::AsyncReadExt, sync::Mutex},
};
use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{self, Row, SqlitePool},
};
use gmv_protocol::{
    avai::{
        feedback::v1::{self as rpc, avai_feedback_server::AvaiFeedback},
        v1::{AiTaskResult, AiTaskState, SourceSpec, source_spec::Source},
    },
    common::v1::NodeIdentity,
};
use prost::Message;
use tonic::{Request, Response, Status};

const PAGE_MAX: u32 = 100;
const SOURCE_REF_MAX: usize = 512;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "feedback", check)]
pub struct FeedbackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_spool_root")]
    pub spool_root: PathBuf,
    #[serde(default)]
    pub capabilities: HashSet<String>,
    #[serde(default)]
    pub sample_permyriad: u32,
    #[serde(default = "default_max_pending_items")]
    pub max_pending_items: u32,
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
    #[serde(default = "default_ttl_ms")]
    pub ttl_ms: i64,
    #[serde(default = "default_evidence_chunk_bytes")]
    pub evidence_chunk_bytes: usize,
    #[serde(default = "default_ack_receipt_capacity")]
    pub ack_receipt_capacity: u32,
    #[serde(default = "default_ack_receipt_retention_ms")]
    pub ack_receipt_retention_ms: i64,
}

fn default_spool_root() -> PathBuf {
    PathBuf::from("./data/feedback-spool")
}
fn default_max_pending_items() -> u32 {
    1024
}
fn default_max_total_bytes() -> u64 {
    536_870_912
}
fn default_ttl_ms() -> i64 {
    86_400_000
}
fn default_evidence_chunk_bytes() -> usize {
    65_536
}
fn default_ack_receipt_capacity() -> u32 {
    4096
}
fn default_ack_receipt_retention_ms() -> i64 {
    86_400_000
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            spool_root: PathBuf::from("./data/feedback-spool"),
            capabilities: HashSet::new(),
            sample_permyriad: 0,
            max_pending_items: 1024,
            max_total_bytes: 536_870_912,
            ttl_ms: 86_400_000,
            evidence_chunk_bytes: 65_536,
            ack_receipt_capacity: 4096,
            ack_receipt_retention_ms: 86_400_000,
        }
    }
}

impl CheckFromConf for FeedbackConfig {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        self.validate()
            .map_err(|error| FieldCheckError::BizError(error.to_string()))
    }
}

impl FeedbackConfig {
    pub fn validate(&self) -> Result<(), FeedbackError> {
        if self.sample_permyriad > 10_000
            || self.max_pending_items == 0
            || self.max_total_bytes == 0
            || self.ttl_ms <= 0
            || self.evidence_chunk_bytes == 0
            || self.evidence_chunk_bytes > 1024 * 1024
            || self.ack_receipt_capacity == 0
            || self.ack_receipt_retention_ms <= 0
            || self.spool_root.as_os_str().is_empty()
        {
            return Err(FeedbackError::new("invalid_feedback_config"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FeedbackError {
    pub code: &'static str,
}

impl FeedbackError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }
    fn storage(error: impl std::fmt::Display) -> Self {
        base::log::warn!(
            "AVAI feedback unavailable: action=feedback, reason=storage_failure, error={error}"
        );
        Self::new("feedback_storage_failure")
    }
}

impl std::fmt::Display for FeedbackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}
impl std::error::Error for FeedbackError {}

type FeedbackResult<T> = Result<T, FeedbackError>;

#[derive(Clone)]
pub struct FeedbackManager {
    config: Arc<FeedbackConfig>,
    pool: SqlitePool,
    root: PathBuf,
    lane: Arc<Mutex<()>>,
    installation_id: String,
    host_id: String,
    identity: NodeIdentity,
}

pub(crate) struct FeedbackMaterial {
    pub task_id: String,
    pub request_hash: String,
    pub route_id: String,
    pub capability: String,
    pub source_ref: String,
    pub result: AiTaskResult,
    pub evidence: Bytes,
    pub evidence_sha256: String,
    pub evidence_media_type: String,
}

pub fn sampled(
    config: &FeedbackConfig,
    capability: &str,
    task_id: &str,
    request_hash: &str,
) -> bool {
    if !config.enabled || !config.capabilities.contains(capability) || config.sample_permyriad == 0
    {
        return false;
    }
    let digest = canonical_hash(
        b"feedback-sampling-v1",
        &[task_id.as_bytes(), request_hash.as_bytes()],
    );
    let first = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 has eight bytes"));
    first % 10_000 < u64::from(config.sample_permyriad)
}

fn canonical_hash(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

pub fn safe_source_ref(source: &SourceSpec) -> Option<String> {
    let value = match source.source.as_ref()? {
        Source::ImageUrl(url_source) => {
            let mut url = url::Url::parse(&url_source.url).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            url.set_query(None);
            url.set_fragment(None);
            url.set_username("").ok()?;
            url.set_password(None).ok()?;
            url.to_string()
        }
        Source::OwnedImage(owned) => {
            let owner = &owned.owner.as_ref()?.node_id;
            let resource = owned.resource.as_ref()?;
            for segment in [
                owner.as_str(),
                &resource.resource_type,
                &resource.resource_id,
            ] {
                if segment.is_empty()
                    || !segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                {
                    return None;
                }
            }
            format!(
                "owned:{owner}/{}/{}",
                resource.resource_type, resource.resource_id
            )
        }
        Source::StreamFrame(_) => return None,
    };
    (value.len() <= SOURCE_REF_MAX).then_some(value)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn owned_spool_filename(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let stem = name
        .strip_suffix(".evidence")
        .or_else(|| name.strip_suffix(".tmp"));
    stem.and_then(|stem| stem.split_once('_'))
        .is_some_and(|(id, sha)| valid_hash(id) && valid_hash(sha))
}

async fn remove_regular_file(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path).await
        && metadata.file_type().is_file()
    {
        let _ = fs::remove_file(path).await;
    }
}

impl FeedbackManager {
    pub async fn open(
        database_path: &Path,
        config: FeedbackConfig,
        installation_id: String,
        host_id: String,
        identity: NodeIdentity,
    ) -> FeedbackResult<Self> {
        config.validate()?;
        if config
            .spool_root
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(FeedbackError::new("invalid_feedback_config"));
        }
        let root = std::path::absolute(&config.spool_root).map_err(FeedbackError::storage)?;
        fs::create_dir_all(&root)
            .await
            .map_err(FeedbackError::storage)?;
        let metadata = fs::symlink_metadata(&root)
            .await
            .map_err(FeedbackError::storage)?;
        if !metadata.file_type().is_dir() {
            return Err(FeedbackError::new("invalid_feedback_config"));
        }
        let root = fs::canonicalize(&root)
            .await
            .map_err(FeedbackError::storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(FeedbackError::storage)?;
        }
        let pool = base_db::dbx::sqlitex::build_sqlite_pool(
            SqliteConnectionConfig::new(database_path),
            DatabasePoolConfig {
                max_size: 4,
                min_idle: Some(1),
                ..Default::default()
            },
        )
        .map_err(FeedbackError::storage)?;
        sqlx::query("CREATE TABLE IF NOT EXISTS avai_feedback_outbox (feedback_id TEXT PRIMARY KEY NOT NULL, content_hash TEXT NOT NULL, task_id TEXT NOT NULL, package BLOB NOT NULL, evidence_sha256 TEXT NOT NULL, evidence_size_bytes INTEGER NOT NULL, evidence_media_type TEXT NOT NULL, state TEXT NOT NULL, created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL)")
            .execute(&pool).await.map_err(FeedbackError::storage)?;
        sqlx::query("CREATE INDEX IF NOT EXISTS avai_feedback_pending_order ON avai_feedback_outbox(state,created_at_ms,feedback_id)")
            .execute(&pool).await.map_err(FeedbackError::storage)?;
        sqlx::query("CREATE TABLE IF NOT EXISTS avai_feedback_ack_receipt (feedback_id TEXT PRIMARY KEY NOT NULL, content_hash TEXT NOT NULL, terminal_outcome INTEGER NOT NULL, acked_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL)")
            .execute(&pool).await.map_err(FeedbackError::storage)?;
        let manager = Self {
            config: Arc::new(config),
            pool,
            root,
            lane: Arc::new(Mutex::new(())),
            installation_id,
            host_id,
            identity,
        };
        manager.recover().await?;
        Ok(manager)
    }

    #[cfg(test)]
    pub(crate) async fn hold_lane_for_test(&self) -> base::tokio::sync::OwnedMutexGuard<()> {
        self.lane.clone().lock_owned().await
    }

    pub fn config(&self) -> &FeedbackConfig {
        &self.config
    }

    fn evidence_path(&self, id: &str, sha: &str) -> FeedbackResult<PathBuf> {
        if !valid_hash(id) || !valid_hash(sha) {
            return Err(FeedbackError::new("invalid_feedback_identity"));
        }
        Ok(self.root.join(format!("{id}_{sha}.evidence")))
    }

    async fn cleanup(&self) -> FeedbackResult<()> {
        let expired = sqlx::query(
            "SELECT feedback_id,evidence_sha256 FROM avai_feedback_outbox WHERE expires_at_ms<=?",
        )
        .bind(now_ms())
        .fetch_all(&self.pool)
        .await
        .map_err(FeedbackError::storage)?;
        sqlx::query("DELETE FROM avai_feedback_outbox WHERE expires_at_ms<=?")
            .bind(now_ms())
            .execute(&self.pool)
            .await
            .map_err(FeedbackError::storage)?;
        sqlx::query("DELETE FROM avai_feedback_ack_receipt WHERE expires_at_ms<=?")
            .bind(now_ms())
            .execute(&self.pool)
            .await
            .map_err(FeedbackError::storage)?;
        for row in expired {
            let id: String = row.try_get("feedback_id").map_err(FeedbackError::storage)?;
            let sha: String = row
                .try_get("evidence_sha256")
                .map_err(FeedbackError::storage)?;
            if let Ok(path) = self.evidence_path(&id, &sha) {
                remove_regular_file(&path).await;
            }
        }
        Ok(())
    }

    pub async fn sweep(&self) -> FeedbackResult<()> {
        self.recover().await
    }

    pub async fn recover(&self) -> FeedbackResult<()> {
        let _guard = self
            .lane
            .try_lock()
            .map_err(|_| FeedbackError::new("feedback_lane_busy"))?;
        self.cleanup().await?;
        let rows = sqlx::query("SELECT o.feedback_id,o.content_hash,o.task_id,o.package,o.evidence_sha256,o.state,t.state AS task_state,t.execution_binding,t.result,t.capability,t.route_id FROM avai_feedback_outbox o LEFT JOIN avai_task t ON t.task_id=o.task_id")
            .fetch_all(&self.pool).await.map_err(FeedbackError::storage)?;
        let mut live_files = HashSet::new();
        for row in rows {
            let id: String = row.try_get("feedback_id").map_err(FeedbackError::storage)?;
            let sha: String = row
                .try_get("evidence_sha256")
                .map_err(FeedbackError::storage)?;
            let path = match self.evidence_path(&id, &sha) {
                Ok(path) => path,
                Err(_) => {
                    sqlx::query("DELETE FROM avai_feedback_outbox WHERE feedback_id=?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await
                        .map_err(FeedbackError::storage)?;
                    continue;
                }
            };
            let encoded: Vec<u8> = row.try_get("package").map_err(FeedbackError::storage)?;
            let package = rpc::FeedbackPackage::decode(encoded.as_slice()).ok();
            let result: Option<Vec<u8>> = row.try_get("result").map_err(FeedbackError::storage)?;
            let binding: Option<Vec<u8>> = row
                .try_get("execution_binding")
                .map_err(FeedbackError::storage)?;
            let state: Option<i32> = row.try_get("task_state").map_err(FeedbackError::storage)?;
            if state == Some(AiTaskState::Running as i32)
                && row
                    .try_get::<String, _>("state")
                    .map_err(FeedbackError::storage)?
                    == "PREPARED"
            {
                live_files.insert(path);
                continue;
            }
            let coherent = package
                .as_ref()
                .zip(result.as_ref())
                .zip(binding.as_ref())
                .and_then(|((package, result), binding)| {
                    let result = AiTaskResult::decode(result.as_slice()).ok()?;
                    let binding: base::serde_json::Value =
                        base::serde_json::from_slice(binding).ok()?;
                    let output = result.output.as_ref()?;
                    let model = result.actual_model.as_ref()?;
                    Some(
                        package.feedback_id == id
                            && package.content_hash
                                == row.try_get::<String, _>("content_hash").ok()?
                            && valid_hash(&package.content_hash)
                            && package_hash(package) == package.content_hash
                            && package.task_id == row.try_get::<String, _>("task_id").ok()?
                            && package.capability == row.try_get::<String, _>("capability").ok()?
                            && package.route_id == row.try_get::<String, _>("route_id").ok()?
                            && package.evidence_sha256 == sha
                            && package.result_sha256
                                == format!("{:x}", Sha256::digest(&output.json))
                            && package.result_schema_name == output.schema
                            && package.result_schema_version == output.version
                            && package.actual_model.as_ref() == Some(model)
                            && binding["capability"] == package.capability
                            && binding["result_schema_name"] == output.schema
                            && binding["result_schema_version"] == output.version
                            && binding["model_id"] == model.model_id
                            && binding["model_version"] == model.version
                            && binding["revision"] == model.revision
                            && binding["runtime"] == model.runtime,
                    )
                })
                .unwrap_or(false);
            if state == Some(AiTaskState::Succeeded as i32)
                && coherent
                && self
                    .file_valid(
                        &path,
                        &sha,
                        package
                            .as_ref()
                            .map_or(0, |value| value.evidence_size_bytes),
                    )
                    .await
            {
                if row
                    .try_get::<String, _>("state")
                    .map_err(FeedbackError::storage)?
                    == "PREPARED"
                {
                    sqlx::query("UPDATE avai_feedback_outbox SET state='PENDING' WHERE feedback_id=? AND state='PREPARED'")
                        .bind(&id).execute(&self.pool).await.map_err(FeedbackError::storage)?;
                }
                live_files.insert(path);
            } else {
                sqlx::query("DELETE FROM avai_feedback_outbox WHERE feedback_id=?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await
                    .map_err(FeedbackError::storage)?;
                remove_regular_file(&path).await;
            }
        }
        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(FeedbackError::storage)?;
        while let Some(entry) = entries.next_entry().await.map_err(FeedbackError::storage)? {
            let path = entry.path();
            if entry
                .file_type()
                .await
                .map_err(FeedbackError::storage)?
                .is_file()
                && owned_spool_filename(&entry.file_name())
                && !live_files.contains(&path)
            {
                remove_regular_file(&path).await;
            }
        }
        Ok(())
    }

    async fn file_valid(&self, path: &Path, sha: &str, size: u64) -> bool {
        if !fs::symlink_metadata(path)
            .await
            .is_ok_and(|metadata| metadata.file_type().is_file())
        {
            return false;
        }
        let Ok(mut file) = fs::File::open(path).await else {
            return false;
        };
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = vec![0; self.config.evidence_chunk_bytes];
        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(count) => {
                    total = total.saturating_add(count as u64);
                    if total > size {
                        return false;
                    }
                    hash.update(&buffer[..count]);
                }
                Err(_) => return false,
            }
        }
        total == size && format!("{:x}", hash.finalize()) == sha
    }

    pub(crate) async fn prepare(
        &self,
        material: FeedbackMaterial,
    ) -> FeedbackResult<Option<String>> {
        if !self.config.enabled {
            return Ok(None);
        }
        let _guard = self
            .lane
            .try_lock()
            .map_err(|_| FeedbackError::new("feedback_lane_busy"))?;
        self.cleanup().await?;
        let size = material.evidence.len() as u64;
        if size == 0
            || size > self.config.max_total_bytes
            || !valid_hash(&material.evidence_sha256)
            || format!("{:x}", Sha256::digest(&material.evidence)) != material.evidence_sha256
            || material.source_ref.len() > SOURCE_REF_MAX
        {
            return Err(FeedbackError::new("invalid_feedback_material"));
        }
        let id = canonical_hash(
            b"feedback-id-v1",
            &[
                material.task_id.as_bytes(),
                material.request_hash.as_bytes(),
            ],
        )
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
        let now = material.result.completed_at_epoch_ms;
        let output = material
            .result
            .output
            .as_ref()
            .ok_or(FeedbackError::new("invalid_feedback_result"))?;
        let model = material
            .result
            .actual_model
            .as_ref()
            .ok_or(FeedbackError::new("invalid_feedback_result"))?;
        let mut package = rpc::FeedbackPackage {
            feedback_id: id.clone(),
            content_hash: String::new(),
            origin_kind: rpc::FeedbackOriginKind::AvaiRuntime as i32,
            trigger_kind: rpc::FeedbackTriggerKind::Sampled as i32,
            installation_id: self.installation_id.clone(),
            host_id: self.host_id.clone(),
            avai_node_id: self.identity.node_id.clone(),
            avai_instance_id: self.identity.instance_id.clone(),
            task_id: material.task_id,
            route_id: material.route_id,
            capability: material.capability,
            source_ref: material.source_ref,
            result_schema_name: output.schema.clone(),
            result_schema_version: output.version,
            result_sha256: format!("{:x}", Sha256::digest(&output.json)),
            actual_model: Some(model.clone()),
            evidence_sha256: material.evidence_sha256,
            evidence_size_bytes: size,
            evidence_media_type: material.evidence_media_type,
            completed_at_epoch_ms: material.result.completed_at_epoch_ms,
            created_at_epoch_ms: now,
            expires_at_epoch_ms: now.saturating_add(self.config.ttl_ms),
        };
        if package.encode_to_vec().len() > 16 * 1024 {
            return Err(FeedbackError::new("feedback_metadata_too_large"));
        }
        package.content_hash = package_hash(&package);
        if let Some(existing) =
            sqlx::query("SELECT content_hash FROM avai_feedback_outbox WHERE feedback_id=?")
                .bind(&id)
                .fetch_optional(&self.pool)
                .await
                .map_err(FeedbackError::storage)?
        {
            let hash: String = existing
                .try_get("content_hash")
                .map_err(FeedbackError::storage)?;
            return if hash == package.content_hash {
                Ok(Some(id))
            } else {
                Err(FeedbackError::new("feedback_identity_conflict"))
            };
        }
        let limits = sqlx::query("SELECT COUNT(*) AS items, COALESCE(SUM(evidence_size_bytes),0) AS bytes FROM avai_feedback_outbox")
            .fetch_one(&self.pool).await.map_err(FeedbackError::storage)?;
        let items: i64 = limits.try_get("items").map_err(FeedbackError::storage)?;
        let bytes: i64 = limits.try_get("bytes").map_err(FeedbackError::storage)?;
        if items >= i64::from(self.config.max_pending_items)
            || (bytes as u64).saturating_add(size) > self.config.max_total_bytes
        {
            return Err(FeedbackError::new("feedback_spool_full"));
        }
        let path = self.evidence_path(&id, &package.evidence_sha256)?;
        let temp = self
            .root
            .join(format!("{id}_{}.tmp", package.evidence_sha256));
        let write_result = async {
            use base::tokio::io::AsyncWriteExt;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)
                .await?;
            file.write_all(&material.evidence).await?;
            file.sync_all().await?;
            fs::rename(&temp, &path).await?;
            Ok::<(), std::io::Error>(())
        }
        .await;
        if let Err(error) = write_result {
            remove_regular_file(&temp).await;
            return Err(FeedbackError::storage(error));
        }
        let inserted = sqlx::query("INSERT INTO avai_feedback_outbox(feedback_id,content_hash,task_id,package,evidence_sha256,evidence_size_bytes,evidence_media_type,state,created_at_ms,expires_at_ms) VALUES(?,?,?,?,?,?,?,'PREPARED',?,?)")
            .bind(&id).bind(&package.content_hash).bind(&package.task_id).bind(package.encode_to_vec())
            .bind(&package.evidence_sha256).bind(size as i64).bind(&package.evidence_media_type)
            .bind(now).bind(package.expires_at_epoch_ms).execute(&self.pool).await;
        if let Err(error) = inserted {
            remove_regular_file(&path).await;
            return Err(FeedbackError::storage(error));
        }
        Ok(Some(id))
    }

    pub async fn promote(&self, id: &str) -> FeedbackResult<()> {
        sqlx::query("UPDATE avai_feedback_outbox SET state='PENDING' WHERE feedback_id=? AND state='PREPARED' AND EXISTS(SELECT 1 FROM avai_task WHERE avai_task.task_id=avai_feedback_outbox.task_id AND avai_task.state=?)")
            .bind(id).bind(AiTaskState::Succeeded as i32).execute(&self.pool).await.map_err(FeedbackError::storage)?;
        Ok(())
    }

    pub async fn discard(&self, id: &str) -> FeedbackResult<()> {
        let _guard = self.lane.lock().await;
        let row = sqlx::query("SELECT evidence_sha256 FROM avai_feedback_outbox WHERE feedback_id=? AND state='PREPARED'")
            .bind(id).fetch_optional(&self.pool).await.map_err(FeedbackError::storage)?;
        if let Some(row) = row {
            let sha: String = row
                .try_get("evidence_sha256")
                .map_err(FeedbackError::storage)?;
            sqlx::query(
                "DELETE FROM avai_feedback_outbox WHERE feedback_id=? AND state='PREPARED'",
            )
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(FeedbackError::storage)?;
            if let Ok(path) = self.evidence_path(id, &sha) {
                remove_regular_file(&path).await;
            }
        }
        Ok(())
    }

    async fn list(
        &self,
        request: rpc::ListPendingFeedbackRequest,
    ) -> FeedbackResult<rpc::ListPendingFeedbackResponse> {
        if request.page_token.len() > 256 {
            return Err(FeedbackError::new("invalid_page_token"));
        }
        let size = if request.page_size == 0 {
            50
        } else {
            request.page_size
        };
        if size > PAGE_MAX {
            return Err(FeedbackError::new("invalid_page_size"));
        }
        let cursor = if request.page_token.is_empty() {
            None
        } else {
            let bytes = URL_SAFE_NO_PAD
                .decode(request.page_token)
                .map_err(|_| FeedbackError::new("invalid_page_token"))?;
            let (time, id): (i64, String) = base::serde_json::from_slice(&bytes)
                .map_err(|_| FeedbackError::new("invalid_page_token"))?;
            if !valid_hash(&id) {
                return Err(FeedbackError::new("invalid_page_token"));
            }
            Some((time, id))
        };
        let (time, id) = cursor.unwrap_or((i64::MIN, String::new()));
        let rows = sqlx::query("SELECT package,created_at_ms,feedback_id FROM avai_feedback_outbox WHERE state='PENDING' AND expires_at_ms>? AND (created_at_ms>? OR (created_at_ms=? AND feedback_id>?)) ORDER BY created_at_ms,feedback_id LIMIT ?")
            .bind(now_ms()).bind(time).bind(time).bind(&id).bind(i64::from(size) + 1)
            .fetch_all(&self.pool).await.map_err(FeedbackError::storage)?;
        let has_more = rows.len() > size as usize;
        let mut packages = Vec::new();
        let mut last = None;
        for row in rows.into_iter().take(size as usize) {
            let encoded: Vec<u8> = row.try_get("package").map_err(FeedbackError::storage)?;
            let package =
                rpc::FeedbackPackage::decode(encoded.as_slice()).map_err(FeedbackError::storage)?;
            last = Some((
                row.try_get::<i64, _>("created_at_ms")
                    .map_err(FeedbackError::storage)?,
                row.try_get::<String, _>("feedback_id")
                    .map_err(FeedbackError::storage)?,
            ));
            packages.push(package);
        }
        let next_page_token = if has_more {
            URL_SAFE_NO_PAD.encode(
                base::serde_json::to_vec(&last.expect("nonempty page"))
                    .map_err(FeedbackError::storage)?,
            )
        } else {
            String::new()
        };
        Ok(rpc::ListPendingFeedbackResponse {
            packages,
            next_page_token,
        })
    }

    async fn read(&self, request: rpc::ReadFeedbackEvidenceRequest) -> FeedbackResult<fs::File> {
        let row = sqlx::query("SELECT content_hash,evidence_sha256,evidence_size_bytes FROM avai_feedback_outbox WHERE feedback_id=? AND state='PENDING' AND expires_at_ms>?")
            .bind(&request.feedback_id).bind(now_ms()).fetch_optional(&self.pool).await.map_err(FeedbackError::storage)?
            .ok_or(FeedbackError::new("feedback_not_found"))?;
        let hash: String = row
            .try_get("content_hash")
            .map_err(FeedbackError::storage)?;
        let sha: String = row
            .try_get("evidence_sha256")
            .map_err(FeedbackError::storage)?;
        let size: i64 = row
            .try_get("evidence_size_bytes")
            .map_err(FeedbackError::storage)?;
        if hash != request.content_hash
            || sha != request.evidence_sha256
            || size < 0
            || size as u64 != request.expected_size
        {
            return Err(FeedbackError::new("feedback_identity_conflict"));
        }
        fs::File::open(self.evidence_path(&request.feedback_id, &sha)?)
            .await
            .map_err(|_| FeedbackError::new("feedback_evidence_unavailable"))
    }

    async fn ack(&self, request: rpc::AckFeedbackRequest) -> FeedbackResult<bool> {
        if !matches!(
            rpc::FeedbackTerminalOutcome::try_from(request.terminal_outcome),
            Ok(rpc::FeedbackTerminalOutcome::Accepted
                | rpc::FeedbackTerminalOutcome::Replayed
                | rpc::FeedbackTerminalOutcome::RejectedPolicy
                | rpc::FeedbackTerminalOutcome::Conflict)
        ) {
            return Err(FeedbackError::new("invalid_terminal_outcome"));
        }
        let _guard = self.lane.lock().await;
        self.cleanup().await?;
        let mut transaction = self.pool.begin().await.map_err(FeedbackError::storage)?;
        let receipt = sqlx::query("SELECT content_hash,terminal_outcome FROM avai_feedback_ack_receipt WHERE feedback_id=?")
            .bind(&request.feedback_id).fetch_optional(&mut *transaction).await.map_err(FeedbackError::storage)?;
        if let Some(receipt) = receipt {
            let hash: String = receipt
                .try_get("content_hash")
                .map_err(FeedbackError::storage)?;
            let outcome: i32 = receipt
                .try_get("terminal_outcome")
                .map_err(FeedbackError::storage)?;
            return if hash == request.content_hash && outcome == request.terminal_outcome {
                Ok(true)
            } else {
                Err(FeedbackError::new("feedback_identity_conflict"))
            };
        }
        let row = sqlx::query("SELECT content_hash,evidence_sha256 FROM avai_feedback_outbox WHERE feedback_id=? AND state='PENDING' AND expires_at_ms>?")
            .bind(&request.feedback_id).bind(now_ms()).fetch_optional(&mut *transaction).await.map_err(FeedbackError::storage)?
            .ok_or(FeedbackError::new("feedback_not_found"))?;
        let hash: String = row
            .try_get("content_hash")
            .map_err(FeedbackError::storage)?;
        let sha: String = row
            .try_get("evidence_sha256")
            .map_err(FeedbackError::storage)?;
        if hash != request.content_hash {
            return Err(FeedbackError::new("feedback_identity_conflict"));
        }
        sqlx::query("DELETE FROM avai_feedback_ack_receipt WHERE feedback_id IN (SELECT feedback_id FROM avai_feedback_ack_receipt ORDER BY acked_at_ms,feedback_id LIMIT MAX((SELECT COUNT(*) FROM avai_feedback_ack_receipt) - ?, 0))")
            .bind(i64::from(self.config.ack_receipt_capacity).saturating_sub(1))
            .execute(&mut *transaction).await.map_err(FeedbackError::storage)?;
        let now = now_ms();
        sqlx::query("INSERT INTO avai_feedback_ack_receipt(feedback_id,content_hash,terminal_outcome,acked_at_ms,expires_at_ms) VALUES(?,?,?,?,?)")
            .bind(&request.feedback_id).bind(&hash).bind(request.terminal_outcome).bind(now)
            .bind(now.saturating_add(self.config.ack_receipt_retention_ms))
            .execute(&mut *transaction).await.map_err(FeedbackError::storage)?;
        sqlx::query("DELETE FROM avai_feedback_outbox WHERE feedback_id=?")
            .bind(&request.feedback_id)
            .execute(&mut *transaction)
            .await
            .map_err(FeedbackError::storage)?;
        transaction.commit().await.map_err(FeedbackError::storage)?;
        if let Ok(path) = self.evidence_path(&request.feedback_id, &sha) {
            remove_regular_file(&path).await;
        }
        Ok(false)
    }
}

fn package_hash(package: &rpc::FeedbackPackage) -> String {
    let mut material = package.clone();
    material.content_hash.clear();
    format!("{:x}", Sha256::digest(material.encode_to_vec()))
}

#[derive(Clone)]
pub struct AvaiFeedbackRpc {
    manager: Option<FeedbackManager>,
}

impl AvaiFeedbackRpc {
    pub fn new(manager: Option<FeedbackManager>) -> Self {
        Self { manager }
    }
}

fn status(error: FeedbackError) -> Status {
    match error.code {
        "feedback_not_found" | "feedback_evidence_unavailable" => Status::not_found(error.code),
        "feedback_identity_conflict" => Status::failed_precondition(error.code),
        "invalid_page_size"
        | "invalid_page_token"
        | "invalid_terminal_outcome"
        | "invalid_feedback_identity" => Status::invalid_argument(error.code),
        _ => Status::unavailable(error.code),
    }
}

#[tonic::async_trait]
impl AvaiFeedback for AvaiFeedbackRpc {
    async fn get_feedback_capabilities(
        &self,
        _request: Request<rpc::GetFeedbackCapabilitiesRequest>,
    ) -> Result<Response<rpc::GetFeedbackCapabilitiesResponse>, Status> {
        let config = self
            .manager
            .as_ref()
            .map(FeedbackManager::config)
            .cloned()
            .unwrap_or_default();
        Ok(Response::new(rpc::GetFeedbackCapabilitiesResponse {
            contract_version: 1,
            enabled: config.enabled,
            max_page_size: PAGE_MAX,
            max_evidence_chunk_bytes: config.evidence_chunk_bytes as u32,
            max_evidence_bytes: config.max_total_bytes,
            supported_trigger_kinds: vec![rpc::FeedbackTriggerKind::Sampled as i32],
        }))
    }

    async fn list_pending_feedback(
        &self,
        request: Request<rpc::ListPendingFeedbackRequest>,
    ) -> Result<Response<rpc::ListPendingFeedbackResponse>, Status> {
        let manager = self
            .manager
            .as_ref()
            .ok_or(Status::unavailable("feedback_disabled"))?;
        Ok(Response::new(
            manager.list(request.into_inner()).await.map_err(status)?,
        ))
    }

    type ReadFeedbackEvidenceStream =
        Pin<Box<dyn Stream<Item = Result<rpc::ReadFeedbackEvidenceResponse, Status>> + Send>>;

    async fn read_feedback_evidence(
        &self,
        request: Request<rpc::ReadFeedbackEvidenceRequest>,
    ) -> Result<Response<Self::ReadFeedbackEvidenceStream>, Status> {
        let manager = self
            .manager
            .as_ref()
            .ok_or(Status::unavailable("feedback_disabled"))?;
        let request = request.into_inner();
        let file = manager.read(request.clone()).await.map_err(status)?;
        let chunk_size = manager.config.evidence_chunk_bytes;
        let stream = base::futures::stream::unfold(
            (file, Sha256::new(), 0u64, request),
            move |(mut file, mut hash, total, request)| async move {
                if total > request.expected_size {
                    return None;
                }
                let mut buffer = vec![0u8; chunk_size];
                match file.read(&mut buffer).await {
                    Ok(0) => {
                        if total != request.expected_size
                            || format!("{:x}", hash.finalize()) != request.evidence_sha256
                        {
                            Some((
                                Err(Status::data_loss("feedback_evidence_mismatch")),
                                (file, Sha256::new(), request.expected_size + 1, request),
                            ))
                        } else {
                            None
                        }
                    }
                    Ok(count) if total.saturating_add(count as u64) <= request.expected_size => {
                        hash.update(&buffer[..count]);
                        buffer.truncate(count);
                        Some((
                            Ok(rpc::ReadFeedbackEvidenceResponse { chunk: buffer }),
                            (file, hash, total + count as u64, request),
                        ))
                    }
                    Ok(_) | Err(_) => Some((
                        Err(Status::data_loss("feedback_evidence_mismatch")),
                        (file, Sha256::new(), request.expected_size + 1, request),
                    )),
                }
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }

    async fn ack_feedback(
        &self,
        request: Request<rpc::AckFeedbackRequest>,
    ) -> Result<Response<rpc::AckFeedbackResponse>, Status> {
        let manager = self
            .manager
            .as_ref()
            .ok_or(Status::unavailable("feedback_disabled"))?;
        let replayed = manager.ack(request.into_inner()).await.map_err(status)?;
        Ok(Response::new(rpc::AckFeedbackResponse { replayed }))
    }
}
