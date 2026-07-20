#[cfg(feature = "db-sqlite")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "db-sqlite")]
use std::sync::LazyLock;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::Deserialize;
use base::serde_default;
#[cfg(feature = "db-sqlite")]
use base_db::dbx::DatabasePoolConfig;
#[cfg(feature = "db-mysql")]
use base_db::dbx::mysqlx;
#[cfg(feature = "db-sqlite")]
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
#[cfg(feature = "db-mysql")]
use base_db::sqlx::MySqlPool;
#[cfg(feature = "db-sqlite")]
use base_db::sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum SessionDatabaseBackend {
    Mysql,
    Sqlite,
}

impl SessionDatabaseBackend {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "db", check)]
pub struct SessionDatabaseConfig {
    #[serde(default = "default_backend")]
    pub backend: SessionDatabaseBackend,
    #[serde(default)]
    pub sqlite: SessionSqliteConfig,
}

serde_default!(
    default_backend,
    SessionDatabaseBackend,
    SessionDatabaseBackend::Mysql
);

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SessionSqliteConfig {
    #[serde(default = "default_sqlite_path")]
    pub path: PathBuf,
    #[serde(default = "default_sqlite_max_connections")]
    pub max_connections: u32,
}

impl Default for SessionSqliteConfig {
    fn default() -> Self {
        Self {
            path: default_sqlite_path(),
            max_connections: default_sqlite_max_connections(),
        }
    }
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from("./session-gb28181.db")
}

fn default_sqlite_max_connections() -> u32 {
    16
}

impl CheckFromConf for SessionDatabaseConfig {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.backend == SessionDatabaseBackend::Mysql && !cfg!(feature = "db-mysql") {
            return Err(FieldCheckError::BizError(
                "当前二进制未启用 MySQL 数据库支持".to_string(),
            ));
        }
        if self.backend == SessionDatabaseBackend::Sqlite && !cfg!(feature = "db-sqlite") {
            return Err(FieldCheckError::BizError(
                "当前二进制未启用 SQLite 数据库支持".to_string(),
            ));
        }
        if self.backend == SessionDatabaseBackend::Sqlite && self.sqlite.path.as_os_str().is_empty()
        {
            return Err(FieldCheckError::BizError(
                "db.sqlite.path不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

impl SessionDatabaseConfig {
    pub fn get() -> Self {
        Self::conf()
    }
}

#[cfg(feature = "db-sqlite")]
static SQLITE_POOL: LazyLock<SqlitePool> = LazyLock::new(|| {
    let config = SessionDatabaseConfig::get();
    let mut pool = DatabasePoolConfig::default();
    pool.max_size = config.sqlite.max_connections;
    pool.min_idle = Some(0);
    ensure_sqlite_parent(&config.sqlite.path)
        .expect("invalid session sqlite directory configuration");
    build_sqlite_pool(SqliteConnectionConfig::new(config.sqlite.path), pool)
        .expect("invalid session sqlite pool configuration")
});

#[cfg(feature = "db-sqlite")]
pub fn sqlite_pool() -> &'static SqlitePool {
    &*SQLITE_POOL
}

pub fn backend() -> SessionDatabaseBackend {
    SessionDatabaseConfig::get().backend
}

#[cfg(feature = "db-sqlite")]
fn ensure_sqlite_parent(path: &Path) -> GlobalResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            GlobalError::new_sys_error(
                &format!("create session SQLite directory failed: {error}"),
                |msg| error!("{msg}"),
            )
        })?;
    }
    Ok(())
}

#[cfg(feature = "db-mysql")]
pub fn mysql_pool() -> &'static MySqlPool {
    mysqlx::get_conn_by_pool()
}

#[cfg(feature = "db-sqlite")]
const SQLITE_SCHEMA: &str = concat!(
    include_str!("../../schema/sqlite/gb28181_core.sql"),
    "\n",
    include_str!("../../schema/sqlite/gb28181_enum_code_seed.sql")
);
#[cfg(feature = "db-mysql")]
const MYSQL_SCHEMA: &str = concat!(
    include_str!("../../schema/mysql/gb28181_core.sql"),
    "\n",
    include_str!("../../schema/mysql/gb28181_enum_code_seed.sql")
);

pub async fn initialize() -> GlobalResult<()> {
    match backend() {
        #[cfg(feature = "db-mysql")]
        SessionDatabaseBackend::Mysql => {
            base_db::sqlx::raw_sql(MYSQL_SCHEMA)
                .execute(mysql_pool())
                .await
                .hand_log(|msg| error!("{msg}"))?;
            ensure_mysql_playback_columns().await
        }
        #[cfg(not(feature = "db-mysql"))]
        SessionDatabaseBackend::Mysql => {
            Err(backend_not_enabled_global(SessionDatabaseBackend::Mysql))
        }
        #[cfg(feature = "db-sqlite")]
        SessionDatabaseBackend::Sqlite => {
            base_db::sqlx::raw_sql(SQLITE_SCHEMA)
                .execute(sqlite_pool())
                .await
                .hand_log(|msg| error!("{msg}"))?;
            ensure_sqlite_playback_columns().await
        }
        #[cfg(not(feature = "db-sqlite"))]
        SessionDatabaseBackend::Sqlite => {
            Err(backend_not_enabled_global(SessionDatabaseBackend::Sqlite))
        }
    }
}

#[cfg(feature = "db-mysql")]
async fn ensure_mysql_playback_columns() -> GlobalResult<()> {
    let existing: Vec<String> = base_db::sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='gb28181_sip_dialog_session'",
    )
    .fetch_all(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    const COLUMNS: &[(&str, &str)] = &[
        ("playback_id", "varchar(64) NULL"),
        ("playback_start_sec", "bigint NULL"),
        ("playback_end_sec", "bigint NULL"),
        ("playback_generation", "bigint NULL"),
        ("mansrtsp_cseq", "bigint NULL"),
        ("acknowledged_position_sec", "bigint NULL"),
        ("desired_rate_milli", "bigint NULL"),
        ("acknowledged_rate_milli", "bigint NULL"),
        ("playback_state", "varchar(16) NULL"),
        ("pause_expire_at", "datetime(3) NULL"),
        ("last_control_operation_id", "varchar(128) NULL"),
        ("registration_epoch_id", "varchar(36) NULL"),
    ];
    for (name, definition) in COLUMNS {
        if !existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_sip_dialog_session ADD COLUMN {name} {definition}"
            )))
            .execute(mysql_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    let device_existing: Vec<String> = base_db::sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='gb28181_device'",
    )
    .fetch_all(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    const DEVICE_COLUMNS: &[(&str, &str)] = &[
        ("registration_call_id", "varchar(128) NULL"),
        ("registration_cseq", "bigint NULL"),
        ("registration_epoch_id", "varchar(36) NULL"),
        ("registration_epoch_closed_at", "datetime(3) NULL"),
    ];
    for (name, definition) in DEVICE_COLUMNS {
        if !device_existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_device ADD COLUMN {name} {definition}"
            )))
            .execute(mysql_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    let epoch_index_exists: Option<i64> = base_db::sqlx::query_scalar(
        "SELECT 1 FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name='gb28181_sip_dialog_session' AND index_name='idx_gmv_sip_dialog_device_epoch_state' LIMIT 1",
    )
    .fetch_optional(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    if epoch_index_exists.is_none() {
        base_db::sqlx::query(
            "CREATE INDEX idx_gmv_sip_dialog_device_epoch_state ON gb28181_sip_dialog_session (device_id, registration_epoch_id, state)",
        )
        .execute(mysql_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    }
    Ok(())
}

#[cfg(feature = "db-sqlite")]
async fn ensure_sqlite_playback_columns() -> GlobalResult<()> {
    use base_db::sqlx::Row;
    let rows = base_db::sqlx::query("PRAGMA table_info(gb28181_sip_dialog_session)")
        .fetch_all(sqlite_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    let existing: Vec<String> = rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    const COLUMNS: &[(&str, &str)] = &[
        ("playback_id", "VARCHAR(64) NULL"),
        ("playback_start_sec", "BIGINT NULL"),
        ("playback_end_sec", "BIGINT NULL"),
        ("playback_generation", "BIGINT NULL"),
        ("mansrtsp_cseq", "BIGINT NULL"),
        ("acknowledged_position_sec", "BIGINT NULL"),
        ("desired_rate_milli", "BIGINT NULL"),
        ("acknowledged_rate_milli", "BIGINT NULL"),
        ("playback_state", "VARCHAR(16) NULL"),
        ("pause_expire_at", "DATETIME NULL"),
        ("last_control_operation_id", "VARCHAR(128) NULL"),
        ("registration_epoch_id", "VARCHAR(36) NULL"),
    ];
    for (name, definition) in COLUMNS {
        if !existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_sip_dialog_session ADD COLUMN {name} {definition}"
            )))
            .execute(sqlite_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    let device_rows = base_db::sqlx::query("PRAGMA table_info(gb28181_device)")
        .fetch_all(sqlite_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    let device_existing: Vec<String> = device_rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    const DEVICE_COLUMNS: &[(&str, &str)] = &[
        ("registration_call_id", "VARCHAR(128) NULL"),
        ("registration_cseq", "BIGINT NULL"),
        ("registration_epoch_id", "VARCHAR(36) NULL"),
        ("registration_epoch_closed_at", "DATETIME NULL"),
    ];
    for (name, definition) in DEVICE_COLUMNS {
        if !device_existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_device ADD COLUMN {name} {definition}"
            )))
            .execute(sqlite_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    base_db::sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_device_epoch_state ON gb28181_sip_dialog_session (device_id, registration_epoch_id, state)",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub(crate) fn backend_not_enabled_global(backend: SessionDatabaseBackend) -> GlobalError {
    GlobalError::new_sys_error(
        &format!(
            "session database backend {} is not enabled in this binary",
            backend.as_str()
        ),
        |msg| error!("{msg}"),
    )
}

pub(crate) fn backend_not_enabled_sqlx(backend: SessionDatabaseBackend) -> base_db::sqlx::Error {
    base_db::sqlx::Error::Protocol(format!(
        "session database backend {} is not enabled in this binary",
        backend.as_str()
    ))
}

#[cfg(all(test, feature = "db-sqlite"))]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use base_db::sqlx::Row;

    #[test]
    fn ensure_sqlite_parent_creates_missing_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("gmv-session-gb28181-sqlite-parent")
            .join(unique.to_string())
            .join("nested");
        let path = dir.join("session-gb28181.db");

        ensure_sqlite_parent(&path).expect("create sqlite parent");

        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir()
                .join("gmv-session-gb28181-sqlite-parent")
                .join(unique.to_string()),
        );
    }

    #[test]
    fn sqlite_schema_uses_lowercase_identifiers() {
        const LEGACY_DB_IDENTIFIERS: &[&str] = &[
            "GB28181_",
            "DEVICE_ID",
            "CHANNEL_ID",
            "ONLINE_EXPIRE_TIME",
            "REGISTER_EXPIRES",
            "REGISTER_TIME",
            "LOCAL_ADDR",
            "CONTACT_URI",
            "ENABLE_LR",
            "CREATE_TIME",
            "UPDATE_TIME",
            "HEARTBEAT_SEC",
            "DOMAIN_ID",
            "PWD_CHECK",
            "BIZ_ID",
            "STREAM_ID",
            "CALL_ID",
            "SIGNAL_NODE_ID",
            "MEDIA_NODE_ID",
        ];

        for identifier in LEGACY_DB_IDENTIFIERS {
            assert!(
                !SQLITE_SCHEMA.contains(identifier),
                "SQLite schema still contains legacy DB identifier {identifier}"
            );
        }
    }

    #[test]
    fn sqlite_schema_initializes_lowercase_database() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let (pool, root) = temp_sqlite_pool("lowercase-schema-init");

            base_db::sqlx::raw_sql(SQLITE_SCHEMA)
                .execute(&pool)
                .await
                .expect("initialize lowercase schema");

            let tables = base_db::sqlx::query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'gb28181_*' ORDER BY name",
            )
            .fetch_all(&pool)
            .await
            .expect("read lowercase tables");
            let names: Vec<String> = tables
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();

            assert!(names.contains(&"gb28181_oauth".to_string()));
            assert!(names.contains(&"gb28181_device".to_string()));
            assert!(names.contains(&"gb28181_sip_dialog_session".to_string()));
            assert!(names.contains(&"gb28181_enum_code".to_string()));
            assert!(names.contains(&"gb28181_resource_confirmation".to_string()));

            let enum_count: i64 = base_db::sqlx::query_scalar(
                "SELECT COUNT(*) FROM gb28181_enum_code WHERE status=1",
            )
            .fetch_one(&pool)
            .await
            .expect("count enum seed rows");
            assert!(enum_count > 100);

            for value in ["111", "118", "131", "132", "136", "137"] {
                let count: i64 = base_db::sqlx::query_scalar(
                    "SELECT COUNT(*) FROM gb28181_enum_code WHERE value_start=? AND value_end=? AND status=1",
                )
                .bind(value)
                .bind(value)
                .fetch_one(&pool)
                .await
                .expect("query enum code");
                assert_eq!(count, 1, "missing exact enum value {value}");
            }

            let legacy_names: i64 = base_db::sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','index') AND name GLOB 'GB28181_*'",
            )
            .fetch_one(&pool)
            .await
            .expect("count legacy names");
            assert_eq!(legacy_names, 0);

            pool.close().await;
            let _ = std::fs::remove_dir_all(root);
        });
    }

    fn temp_sqlite_pool(name: &str) -> (SqlitePool, std::path::PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir()
            .join("gmv-session-gb28181-schema")
            .join(unique.to_string());
        let path = root.join(format!("{name}.db"));
        ensure_sqlite_parent(&path).expect("create sqlite test parent");

        let mut pool = DatabasePoolConfig::default();
        pool.max_size = 1;
        pool.min_idle = Some(0);
        (
            build_sqlite_pool(SqliteConnectionConfig::new(path), pool)
                .expect("create sqlite test pool"),
            root,
        )
    }
}

macro_rules! execute {
    ($sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            #[cfg(feature = "db-mysql")]
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query($sql)
                    $(.bind($bind))*
                    .execute($crate::storage::db::mysql_pool())
                    .await
                    .map(|result| result.rows_affected())
            }
            #[cfg(feature = "db-sqlite")]
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query($sql)
                    $(.bind($bind))*
                    .execute($crate::storage::db::sqlite_pool())
                    .await
                    .map(|result| result.rows_affected())
            }
            backend => Err($crate::storage::db::backend_not_enabled_sqlx(backend)),
        }
    }};
}

macro_rules! fetch_optional_as {
    ($ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            #[cfg(feature = "db-mysql")]
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_optional($crate::storage::db::mysql_pool())
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_optional($crate::storage::db::sqlite_pool())
                    .await
            }
            backend => Err($crate::storage::db::backend_not_enabled_sqlx(backend)),
        }
    }};
}

macro_rules! fetch_all_as {
    ($ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            #[cfg(feature = "db-mysql")]
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_all($crate::storage::db::mysql_pool())
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_all($crate::storage::db::sqlite_pool())
                    .await
            }
            backend => Err($crate::storage::db::backend_not_enabled_sqlx(backend)),
        }
    }};
}

pub(crate) use execute;
pub(crate) use fetch_all_as;
pub(crate) use fetch_optional_as;
