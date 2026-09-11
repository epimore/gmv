use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{Row, SqlitePool},
};

use super::{
    ModelError, ModelIdentity, ModelResult, VerifiedModelPackage, package::safe_relative_path,
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
    pub resources: super::ResourceHints,
    pub self_tests: Vec<super::SelfTestCase>,
    pub state: ModelState,
    pub active_generation: Option<u64>,
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
        Ok(Self {
            pool,
            packages_root: Arc::new(packages_root),
            quarantine_root: Arc::new(quarantine_root),
        })
    }

    pub async fn install(
        &self,
        package: &VerifiedModelPackage,
        now_epoch_ms: i64,
    ) -> ModelResult<InstalledModel> {
        if let Some(existing) = self.get(&package.manifest.metadata).await? {
            if existing.manifest_sha256 == package.manifest_sha256 {
                return Ok(existing);
            }
            return Err(ModelError::new(
                "model_revision_conflict",
                "immutable model revision already exists with different content",
            ));
        }
        let identity = &package.manifest.metadata;
        let destination = self
            .packages_root
            .join(&identity.model_id)
            .join(&identity.version)
            .join(&identity.revision);
        if destination.exists() {
            return Err(ModelError::new(
                "model_revision_conflict",
                "model revision directory already exists without matching metadata",
            ));
        }
        let staging = destination
            .parent()
            .unwrap_or(&self.packages_root)
            .join(format!(
                ".{}.installing-{}",
                identity.revision,
                std::process::id()
            ));
        if staging.exists() {
            return Err(ModelError::new(
                "model_install_in_progress",
                "model revision staging directory already exists",
            ));
        }
        create_directory(&staging)?;
        let install_result = (|| -> ModelResult<()> {
            copy_confined_file(
                &package.root,
                Path::new("manifest.yaml"),
                &staging.join("manifest.yaml"),
            )?;
            for file in &package.manifest.files {
                let relative = safe_relative_path(&file.path)?;
                copy_confined_file(&package.root, &relative, &staging.join(&relative))?;
            }
            if let Some(parent) = destination.parent() {
                create_directory(parent)?;
            }
            std::fs::rename(&staging, &destination)
                .map_err(|error| ModelError::io("commit immutable model revision", error))?;
            Ok(())
        })();
        if let Err(error) = install_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        let capabilities_json = base::serde_json::to_string(&package.manifest.capabilities)
            .map_err(|error| ModelError::io("encode model capabilities", error))?;
        let self_tests_json = base::serde_json::to_string(&package.manifest.self_test)
            .map_err(|error| ModelError::io("encode model self-tests", error))?;
        let result = base_db::sqlx::query(
            "INSERT INTO avai_model_revision(\
             model_id,version,revision,capabilities_json,runtime,installed_path,manifest_sha256,\
             memory_mb,vram_mb,max_batch,self_tests_json,state,installed_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .bind(capabilities_json)
        .bind(&package.selected_variant.runtime)
        .bind(destination.to_string_lossy().as_ref())
        .bind(&package.manifest_sha256)
        .bind(
            i64::try_from(package.manifest.resources.memory_mb).map_err(|_| {
                ModelError::new(
                    "model_resource_limit_exceeded",
                    "memory hint does not fit SQLite",
                )
            })?,
        )
        .bind(
            i64::try_from(package.manifest.resources.vram_mb).map_err(|_| {
                ModelError::new(
                    "model_resource_limit_exceeded",
                    "VRAM hint does not fit SQLite",
                )
            })?,
        )
        .bind(i64::from(package.manifest.resources.max_batch))
        .bind(self_tests_json)
        .bind(ModelState::Installed as i32)
        .bind(now_epoch_ms)
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&destination);
            return Err(ModelError::io("persist installed model", error));
        }
        self.get(identity).await?.ok_or_else(|| {
            ModelError::new(
                "model_install_failed",
                "installed model metadata was not readable",
            )
        })
    }

    pub async fn get(&self, identity: &ModelIdentity) -> ModelResult<Option<InstalledModel>> {
        base_db::sqlx::query(SELECT_MODEL)
            .bind(&identity.model_id)
            .bind(&identity.version)
            .bind(&identity.revision)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| ModelError::io("query installed model", error))?
            .map(decode_model)
            .transpose()
    }

    pub async fn list(&self) -> ModelResult<Vec<InstalledModel>> {
        let rows = base_db::sqlx::query(SELECT_MODELS)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| ModelError::io("list installed models", error))?;
        rows.into_iter().map(decode_model).collect()
    }

    pub(crate) async fn max_generation(&self) -> ModelResult<u64> {
        let value: i64 = base_db::sqlx::query_scalar(
            "SELECT COALESCE(MAX(active_generation), 0) FROM avai_model_revision",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| ModelError::io("query model generation", error))?;
        u64::try_from(value)
            .map_err(|_| ModelError::new("model_generation_invalid", "negative generation"))
    }

    pub(crate) async fn set_state(
        &self,
        identity: &ModelIdentity,
        state: ModelState,
        generation: Option<u64>,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        let generation = generation
            .map(i64::try_from)
            .transpose()
            .map_err(|_| ModelError::new("model_generation_invalid", "generation is too large"))?;
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?, active_generation=?, activated_at_ms=CASE WHEN ? IS NULL THEN activated_at_ms ELSE ? END, last_error=NULL WHERE model_id=? AND version=? AND revision=?",
        )
        .bind(state as i32)
        .bind(generation)
        .bind(generation)
        .bind(now_epoch_ms)
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("update model state", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_not_found",
                "installed model does not exist",
            ));
        }
        Ok(())
    }

    pub(crate) async fn activate(
        &self,
        identity: &ModelIdentity,
        no_longer_active: &[ModelIdentity],
        generation: u64,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        let generation = i64::try_from(generation)
            .map_err(|_| ModelError::new("model_generation_invalid", "generation is too large"))?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| ModelError::io("begin model activation", error))?;
        for previous in no_longer_active {
            base_db::sqlx::query(
                "UPDATE avai_model_revision SET state=?, active_generation=NULL WHERE model_id=? AND version=? AND revision=?",
            )
            .bind(ModelState::Ready as i32)
            .bind(&previous.model_id)
            .bind(&previous.version)
            .bind(&previous.revision)
            .execute(&mut *transaction)
            .await
            .map_err(|error| ModelError::io("persist previous model state", error))?;
        }
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?, active_generation=?, activated_at_ms=?, last_error=NULL WHERE model_id=? AND version=? AND revision=?",
        )
        .bind(ModelState::Active as i32)
        .bind(generation)
        .bind(now_epoch_ms)
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("persist active model state", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_not_found",
                "installed model does not exist",
            ));
        }
        transaction
            .commit()
            .await
            .map_err(|error| ModelError::io("commit model activation", error))
    }

    pub async fn remove(&self, identity: &ModelIdentity) -> ModelResult<()> {
        let Some(model) = self.get(identity).await? else {
            return Ok(());
        };
        if matches!(model.state, ModelState::Active | ModelState::Ready) {
            return Err(ModelError::new(
                "model_in_use",
                "active or loaded model revision cannot be removed",
            ));
        }
        let expected_path = self
            .packages_root
            .join(&identity.model_id)
            .join(&identity.version)
            .join(&identity.revision);
        if model.installed_path != expected_path {
            return Err(ModelError::new(
                "model_path_invalid",
                "persisted model path does not match its immutable identity",
            ));
        }
        let quarantine = self.quarantine_root.join(format!(
            ".{}-{}-{}.removing-{}",
            identity.model_id,
            identity.version,
            identity.revision,
            std::process::id()
        ));
        std::fs::rename(&model.installed_path, &quarantine)
            .map_err(|error| ModelError::io("quarantine model before removal", error))?;
        let deleted = base_db::sqlx::query(
            "DELETE FROM avai_model_revision WHERE model_id=? AND version=? AND revision=? AND state NOT IN (?,?)",
        )
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .bind(ModelState::Active as i32)
        .bind(ModelState::Ready as i32)
        .execute(&self.pool)
        .await;
        let deleted = match deleted {
            Ok(deleted) => deleted,
            Err(error) => {
                let _ = std::fs::rename(&quarantine, &model.installed_path);
                return Err(ModelError::io("delete model metadata", error));
            }
        };
        if deleted.rows_affected() != 1 {
            let _ = std::fs::rename(&quarantine, &model.installed_path);
            return Err(ModelError::new(
                "model_in_use",
                "model state changed during removal",
            ));
        }
        std::fs::remove_dir_all(&quarantine)
            .map_err(|error| ModelError::io("remove quarantined model files", error))?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

const SELECT_MODEL: &str = "SELECT model_id,version,revision,capabilities_json,runtime,installed_path,manifest_sha256,memory_mb,vram_mb,max_batch,self_tests_json,state,active_generation FROM avai_model_revision WHERE model_id=? AND version=? AND revision=?";
const SELECT_MODELS: &str = "SELECT model_id,version,revision,capabilities_json,runtime,installed_path,manifest_sha256,memory_mb,vram_mb,max_batch,self_tests_json,state,active_generation FROM avai_model_revision ORDER BY model_id,version,revision";

fn decode_model(row: base_db::sqlx::sqlite::SqliteRow) -> ModelResult<InstalledModel> {
    let capabilities_json: String = row
        .try_get("capabilities_json")
        .map_err(|error| ModelError::io("decode model capabilities", error))?;
    let self_tests_json: String = row
        .try_get("self_tests_json")
        .map_err(|error| ModelError::io("decode model self-tests", error))?;
    let memory_mb: i64 = row.try_get("memory_mb").map_err(decode_error)?;
    let vram_mb: i64 = row.try_get("vram_mb").map_err(decode_error)?;
    let max_batch: i64 = row.try_get("max_batch").map_err(decode_error)?;
    let active_generation: Option<i64> = row.try_get("active_generation").map_err(decode_error)?;
    Ok(InstalledModel {
        identity: ModelIdentity {
            model_id: row.try_get("model_id").map_err(decode_error)?,
            version: row.try_get("version").map_err(decode_error)?,
            revision: row.try_get("revision").map_err(decode_error)?,
        },
        capabilities: base::serde_json::from_str(&capabilities_json)
            .map_err(|error| ModelError::io("parse model capabilities", error))?,
        runtime: row.try_get("runtime").map_err(decode_error)?,
        installed_path: PathBuf::from(
            row.try_get::<String, _>("installed_path")
                .map_err(decode_error)?,
        ),
        manifest_sha256: row.try_get("manifest_sha256").map_err(decode_error)?,
        resources: super::ResourceHints {
            memory_mb: u64::try_from(memory_mb).map_err(|_| invalid_resource())?,
            vram_mb: u64::try_from(vram_mb).map_err(|_| invalid_resource())?,
            max_batch: u32::try_from(max_batch).map_err(|_| invalid_resource())?,
        },
        self_tests: base::serde_json::from_str(&self_tests_json)
            .map_err(|error| ModelError::io("parse model self-tests", error))?,
        state: ModelState::try_from(row.try_get::<i32, _>("state").map_err(decode_error)?)?,
        active_generation: active_generation
            .map(u64::try_from)
            .transpose()
            .map_err(|_| ModelError::new("model_generation_invalid", "negative generation"))?,
    })
}

fn decode_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::io("decode installed model", error)
}

fn invalid_resource() -> ModelError {
    ModelError::new(
        "model_resource_invalid",
        "persisted resource hint is invalid",
    )
}

fn create_directory(path: &Path) -> ModelResult<()> {
    std::fs::create_dir_all(path).map_err(|error| ModelError::io("create model directory", error))
}

fn copy_confined_file(root: &Path, relative: &Path, destination: &Path) -> ModelResult<()> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| ModelError::io("resolve model package root", error))?;
    let source = root.join(relative);
    let canonical_source = source
        .canonicalize()
        .map_err(|error| ModelError::io("resolve model source file", error))?;
    if !canonical_source.starts_with(&canonical_root) {
        return Err(ModelError::new(
            "model_path_invalid",
            "model source resolves outside the package root",
        ));
    }
    let metadata = std::fs::symlink_metadata(&source)
        .map_err(|error| ModelError::io("inspect model source file", error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ModelError::new(
            "invalid_model_package",
            format!("model source is not a regular file: {}", source.display()),
        ));
    }
    if let Some(parent) = destination.parent() {
        create_directory(parent)?;
    }
    std::fs::copy(&canonical_source, destination)
        .map_err(|error| ModelError::io("copy model package file", error))?;
    Ok(())
}
