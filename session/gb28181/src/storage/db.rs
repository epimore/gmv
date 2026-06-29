use std::path::PathBuf;
use std::sync::LazyLock;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::Deserialize;
use base::serde_default;
use base_db::dbx::DatabasePoolConfig;
use base_db::dbx::mysqlx;
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
use base_db::sqlx::{MySqlPool, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum SessionDatabaseBackend {
    Mysql,
    Sqlite,
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

static SQLITE_POOL: LazyLock<SqlitePool> = LazyLock::new(|| {
    let config = SessionDatabaseConfig::get();
    let mut pool = DatabasePoolConfig::default();
    pool.max_size = config.sqlite.max_connections;
    pool.min_idle = Some(0);
    build_sqlite_pool(SqliteConnectionConfig::new(config.sqlite.path), pool)
        .expect("invalid session sqlite pool configuration")
});

pub fn sqlite_pool() -> &'static SqlitePool {
    &*SQLITE_POOL
}

pub fn backend() -> SessionDatabaseBackend {
    SessionDatabaseConfig::get().backend
}

pub fn mysql_pool() -> &'static MySqlPool {
    mysqlx::get_conn_by_pool()
}

const SQLITE_0001: &str = include_str!("../../migrations/sqlite/0001_gb28181_core.sql");
const MYSQL_0001: &str = include_str!("../../migrations/mysql/0001_gb28181_core.sql");

const SQLITE_MIGRATIONS: &[base_db::migration::Migration] = &[base_db::migration::Migration {
    version: 1,
    name: "gb28181_core",
    sql: SQLITE_0001,
}];

const MYSQL_MIGRATIONS: &[base_db::migration::Migration] = &[base_db::migration::Migration {
    version: 1,
    name: "gb28181_core",
    sql: MYSQL_0001,
}];

pub async fn initialize() -> GlobalResult<()> {
    match backend() {
        SessionDatabaseBackend::Mysql => {
            base_db::migration::run_mysql_migrations(mysql_pool(), MYSQL_MIGRATIONS)
                .await
                .hand_log(|msg| error!("{msg}"))
        }
        SessionDatabaseBackend::Sqlite => {
            base_db::migration::run_sqlite_migrations(sqlite_pool(), SQLITE_MIGRATIONS)
                .await
                .hand_log(|msg| error!("{msg}"))
        }
    }
}
macro_rules! execute {
    ($sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query($sql)
                    $(.bind($bind))*
                    .execute($crate::storage::db::mysql_pool())
                    .await
                    .map(|result| result.rows_affected())
            }
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query($sql)
                    $(.bind($bind))*
                    .execute($crate::storage::db::sqlite_pool())
                    .await
                    .map(|result| result.rows_affected())
            }
        }
    }};
}

macro_rules! fetch_optional_as {
    ($ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_optional($crate::storage::db::mysql_pool())
                    .await
            }
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_optional($crate::storage::db::sqlite_pool())
                    .await
            }
        }
    }};
}

macro_rules! fetch_all_as {
    ($ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        match $crate::storage::db::backend() {
            $crate::storage::db::SessionDatabaseBackend::Mysql => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_all($crate::storage::db::mysql_pool())
                    .await
            }
            $crate::storage::db::SessionDatabaseBackend::Sqlite => {
                base_db::sqlx::query_as::<_, $ty>($sql)
                    $(.bind($bind))*
                    .fetch_all($crate::storage::db::sqlite_pool())
                    .await
            }
        }
    }};
}

pub(crate) use execute;
pub(crate) use fetch_all_as;
pub(crate) use fetch_optional_as;
