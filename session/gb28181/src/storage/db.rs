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

#[cfg(feature = "db-mysql")]
pub fn mysql_pool() -> &'static MySqlPool {
    mysqlx::get_conn_by_pool()
}

#[cfg(feature = "db-sqlite")]
const SQLITE_SCHEMA: &str = include_str!("../../schema/sqlite/gb28181_core.sql");
#[cfg(feature = "db-mysql")]
const MYSQL_SCHEMA: &str = include_str!("../../schema/mysql/gb28181_core.sql");

pub async fn initialize() -> GlobalResult<()> {
    match backend() {
        #[cfg(feature = "db-mysql")]
        SessionDatabaseBackend::Mysql => base_db::sqlx::raw_sql(MYSQL_SCHEMA)
            .execute(mysql_pool())
            .await
            .map(|_| ())
            .hand_log(|msg| error!("{msg}")),
        #[cfg(not(feature = "db-mysql"))]
        SessionDatabaseBackend::Mysql => {
            Err(backend_not_enabled_global(SessionDatabaseBackend::Mysql))
        }
        #[cfg(feature = "db-sqlite")]
        SessionDatabaseBackend::Sqlite => base_db::sqlx::raw_sql(SQLITE_SCHEMA)
            .execute(sqlite_pool())
            .await
            .map(|_| ())
            .hand_log(|msg| error!("{msg}")),
        #[cfg(not(feature = "db-sqlite"))]
        SessionDatabaseBackend::Sqlite => {
            Err(backend_not_enabled_global(SessionDatabaseBackend::Sqlite))
        }
    }
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
