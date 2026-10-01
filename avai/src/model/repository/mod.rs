mod operations;
mod revisions;
mod slots;

use revisions::create_directory;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{Row, SqlitePool},
};

use super::{
    ModelError, ModelIdentity, ModelResult, ResourceHints, SelectedRuntimeVariant,
    VerifiedModelPackage, package::safe_relative_path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ModelState {
    Installed = 1,
    Ready = 2,
    Active = 3,
    Retired = 4,
    Failed = 5,
}

impl TryFrom<i32> for ModelState {
    type Error = ModelError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Installed),
            2 => Ok(Self::Ready),
            3 => Ok(Self::Active),
            4 => Ok(Self::Retired),
            5 => Ok(Self::Failed),
            _ => Err(ModelError::new(
                "model_state_invalid",
                format!("unknown persisted model state: {value}"),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub identity: ModelIdentity,
    pub capabilities: Vec<String>,
    pub runtime: String,
    pub installed_path: PathBuf,
    pub manifest_sha256: String,
    pub selected_variant: Option<SelectedRuntimeVariant>,
    pub resources: super::ResourceHints,
    pub self_tests: Vec<super::SelfTestCase>,
    pub state: ModelState,
    pub active_generation: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct PersistedCapabilitySlot {
    pub capability: String,
    pub active_identity: ModelIdentity,
    pub active_generation: u64,
    pub previous_identity: Option<ModelIdentity>,
    pub previous_generation: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct CapabilityRecovery {
    pub capability: String,
    pub expected_active_generation: u64,
    pub replacement: Option<(ModelIdentity, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum OperationReceiptState {
    Pending = 1,
    Succeeded = 2,
    Failed = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationReceipt {
    pub operation_id: String,
    pub idempotency_key: String,
    pub operation_kind: String,
    pub request_hash: String,
    pub state: OperationReceiptState,
    pub stable_error_code: Option<String>,
    pub deadline_epoch_ms: i64,
}

#[derive(Debug)]
pub(crate) enum ClaimOperation {
    New(OperationReceipt),
    Existing(OperationReceipt),
}

pub(crate) struct OperationClaimRequest<'a> {
    pub operation_id: &'a str,
    pub idempotency_key: &'a str,
    pub operation_kind: &'a str,
    pub request_hash: &'a str,
    pub deadline_epoch_ms: i64,
    pub now_epoch_ms: i64,
}

pub(crate) struct OperationReceiptLimits {
    pub retention_ms: i64,
    pub capacity: usize,
}

#[derive(Clone)]
pub struct ModelRepository {
    pool: SqlitePool,
    packages_root: Arc<PathBuf>,
    quarantine_root: Arc<PathBuf>,
}

impl ModelRepository {
    pub async fn open(database_path: &Path, model_root: &Path) -> ModelResult<Self> {
        if let Some(parent) = database_path.parent()
            && !parent.as_os_str().is_empty()
        {
            create_directory(parent)?;
        }
        let packages_root = model_root.join("packages");
        let quarantine_root = model_root.join("quarantine");
        create_directory(&packages_root)?;
        create_directory(&quarantine_root)?;
        let pool = base_db::dbx::sqlitex::build_sqlite_pool(
            SqliteConnectionConfig::new(database_path),
            DatabasePoolConfig {
                max_size: 4,
                min_idle: Some(1),
                ..Default::default()
            },
        )
        .map_err(|error| ModelError::io("configure model database", error))?;
        base_db::sqlx::query(
            "CREATE TABLE IF NOT EXISTS avai_model_revision (\
             model_id TEXT NOT NULL,\
             version TEXT NOT NULL,\
             revision TEXT NOT NULL,\
             capabilities_json TEXT NOT NULL,\
             runtime TEXT NOT NULL,\
             installed_path TEXT NOT NULL,\
             manifest_sha256 TEXT NOT NULL,\
             memory_mb INTEGER NOT NULL,\
             vram_mb INTEGER NOT NULL,\
             max_batch INTEGER NOT NULL,\
             self_tests_json TEXT NOT NULL,\
             state INTEGER NOT NULL,\
             active_generation INTEGER NULL,\
             installed_at_ms INTEGER NOT NULL,\
             activated_at_ms INTEGER NULL,\
             last_error TEXT NULL,\
             PRIMARY KEY(model_id, version, revision)\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|error| ModelError::io("initialize model schema", error))?;
        ensure_selected_variant_column(&pool).await?;
        base_db::sqlx::query(
            "CREATE TABLE IF NOT EXISTS avai_model_capability_slot (\
             capability TEXT NOT NULL PRIMARY KEY,\
             active_model_id TEXT NOT NULL,\
             active_version TEXT NOT NULL,\
             active_revision TEXT NOT NULL,\
             active_generation INTEGER NOT NULL,\
             previous_model_id TEXT NULL,\
             previous_version TEXT NULL,\
             previous_revision TEXT NULL,\
             previous_generation INTEGER NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|error| ModelError::io("initialize model slot schema", error))?;
        base_db::sqlx::query(
            "CREATE TABLE IF NOT EXISTS avai_model_management_operation (\
             operation_id TEXT NOT NULL PRIMARY KEY,\
             idempotency_key TEXT NOT NULL UNIQUE,\
             operation_kind TEXT NOT NULL,\
             request_hash TEXT NOT NULL,\
             state INTEGER NOT NULL,\
             stable_error_code TEXT NULL,\
             created_at_ms INTEGER NOT NULL,\
             updated_at_ms INTEGER NOT NULL,\
             deadline_epoch_ms INTEGER NOT NULL,\
             terminal_at_ms INTEGER NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|error| ModelError::io("initialize model operation receipt schema", error))?;
        Ok(Self {
            pool,
            packages_root: Arc::new(packages_root),
            quarantine_root: Arc::new(quarantine_root),
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

async fn ensure_selected_variant_column(pool: &SqlitePool) -> ModelResult<()> {
    let columns = base_db::sqlx::query("PRAGMA table_info(avai_model_revision)")
        .fetch_all(pool)
        .await
        .map_err(|error| ModelError::io("inspect model schema", error))?;
    if columns.iter().any(|row| {
        row.try_get::<String, _>("name")
            .is_ok_and(|name| name == "selected_variant_json")
    }) {
        return Ok(());
    }
    base_db::sqlx::query(
        "ALTER TABLE avai_model_revision ADD COLUMN selected_variant_json TEXT NULL",
    )
    .execute(pool)
    .await
    .map_err(|error| ModelError::io("migrate selected model variant", error))?;
    Ok(())
}

fn decode_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::io("decode installed model", error)
}
