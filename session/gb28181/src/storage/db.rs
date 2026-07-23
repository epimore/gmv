#[cfg(feature = "db-sqlite")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::Deserialize;
use base::serde_default;
use base::utils::crypto::default_decrypt;
use base_db::dbx::DatabasePoolConfig;
#[cfg(feature = "db-mysql")]
use base_db::dbx::mysqlx::build_mysql_pool;
#[cfg(feature = "db-sqlite")]
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
#[cfg(feature = "db-mysql")]
use base_db::sqlx::ConnectOptions;
#[cfg(feature = "db-mysql")]
use base_db::sqlx::MySqlPool;
#[cfg(feature = "db-sqlite")]
use base_db::sqlx::SqlitePool;
#[cfg(feature = "db-mysql")]
use base_db::sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

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
    pub pool: SessionPoolConfig,
    #[serde(default)]
    pub sqlite: SessionSqliteConfig,
    pub mysql: Option<SessionMysqlConfig>,
}

serde_default!(
    default_backend,
    SessionDatabaseBackend,
    SessionDatabaseBackend::Mysql
);

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SessionPoolConfig {
    #[serde(default = "default_pool_max_connections")]
    pub max_connections: u32,
    #[serde(default)]
    pub min_connections: u32,
    #[serde(default = "default_pool_connection_timeout_sec")]
    pub connection_timeout_sec: u64,
    #[serde(default = "default_pool_max_lifetime_sec")]
    pub max_lifetime_sec: u64,
    #[serde(default = "default_pool_idle_timeout_sec")]
    pub idle_timeout_sec: u64,
    #[serde(default)]
    pub check_health: bool,
}

impl Default for SessionPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: default_pool_max_connections(),
            min_connections: 0,
            connection_timeout_sec: default_pool_connection_timeout_sec(),
            max_lifetime_sec: default_pool_max_lifetime_sec(),
            idle_timeout_sec: default_pool_idle_timeout_sec(),
            check_health: false,
        }
    }
}

impl SessionPoolConfig {
    fn validate(&self) -> Result<(), String> {
        if self.max_connections == 0 || self.min_connections > self.max_connections {
            return Err("db.pool连接数配置无效".to_string());
        }
        if self.connection_timeout_sec == 0 {
            return Err("db.pool.connection_timeout_sec必须大于0".to_string());
        }
        Ok(())
    }

    fn to_base_db(&self) -> DatabasePoolConfig {
        DatabasePoolConfig {
            max_size: self.max_connections,
            min_idle: Some(self.min_connections),
            connection_timeout: Duration::from_secs(self.connection_timeout_sec),
            max_lifetime: Some(Duration::from_secs(self.max_lifetime_sec)),
            idle_timeout: Some(Duration::from_secs(self.idle_timeout_sec)),
            test_on_check_out: self.check_health,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SessionSqliteConfig {
    #[serde(default = "default_sqlite_path")]
    pub path: PathBuf,
}

impl Default for SessionSqliteConfig {
    fn default() -> Self {
        Self {
            path: default_sqlite_path(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SessionMysqlConfig {
    pub host: String,
    #[serde(default = "default_mysql_port")]
    pub port: u16,
    pub database: String,
    pub username: String,
    #[serde(default)]
    pub pass_crypto_enable: bool,
    pub pass: String,
    #[serde(default)]
    pub ssl_mode: SessionMysqlSslMode,
    #[serde(default)]
    pub attrs: SessionMysqlAttrsConfig,
}

impl SessionMysqlConfig {
    fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty()
            || self.port == 0
            || self.database.trim().is_empty()
            || self.username.trim().is_empty()
            || self.pass.is_empty()
        {
            return Err("db.mysql连接字段不能为空".to_string());
        }
        self.attrs.validate()?;
        if self.pass_crypto_enable {
            self.password()?;
        }
        Ok(())
    }

    fn password(&self) -> Result<String, String> {
        if self.pass_crypto_enable {
            default_decrypt(&self.pass).map_err(|error| error.to_string())
        } else {
            Ok(self.pass.clone())
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum SessionMysqlSslMode {
    Disabled,
    #[default]
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct SessionMysqlAttrsConfig {
    pub log_global_sql_level: Option<String>,
    pub log_slow_sql_timeout_sec: Option<u64>,
    pub timezone: Option<String>,
    pub charset: Option<String>,
    pub ssl_ca_crt_file: Option<PathBuf>,
    pub ssl_client_cert_file: Option<PathBuf>,
    pub ssl_client_key_file: Option<PathBuf>,
}

impl SessionMysqlAttrsConfig {
    fn validate(&self) -> Result<(), String> {
        if self.log_slow_sql_timeout_sec == Some(0) {
            return Err("db.mysql.attrs.log_slow_sql_timeout_sec必须大于0".to_string());
        }
        if self.ssl_client_cert_file.is_some() != self.ssl_client_key_file.is_some() {
            return Err(
                "db.mysql.attrs.ssl_client_cert_file和ssl_client_key_file必须同时配置".to_string(),
            );
        }
        Ok(())
    }
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from("./session-gb28181.db")
}

fn default_pool_max_connections() -> u32 {
    16
}

fn default_pool_connection_timeout_sec() -> u64 {
    8
}

fn default_pool_max_lifetime_sec() -> u64 {
    1800
}

fn default_pool_idle_timeout_sec() -> u64 {
    60
}

fn default_mysql_port() -> u16 {
    3306
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
        self.pool.validate().map_err(FieldCheckError::BizError)?;
        if self.backend == SessionDatabaseBackend::Sqlite && self.sqlite.path.as_os_str().is_empty()
        {
            return Err(FieldCheckError::BizError(
                "db.sqlite.path不能为空".to_string(),
            ));
        }
        if self.backend == SessionDatabaseBackend::Mysql {
            self.mysql
                .as_ref()
                .ok_or_else(|| FieldCheckError::BizError("db.mysql不能为空".to_string()))?
                .validate()
                .map_err(FieldCheckError::BizError)?;
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
static SQLITE_POOL: OnceLock<SqlitePool> = OnceLock::new();

#[cfg(feature = "db-mysql")]
static MYSQL_POOL: OnceLock<MySqlPool> = OnceLock::new();

#[cfg(feature = "db-sqlite")]
pub fn sqlite_pool() -> &'static SqlitePool {
    SQLITE_POOL
        .get()
        .expect("session SQLite pool must be initialized before use")
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
    MYSQL_POOL
        .get()
        .expect("session MySQL pool must be initialized before use")
}

#[cfg(feature = "db-sqlite")]
fn initialize_sqlite_pool(config: &SessionDatabaseConfig) -> GlobalResult<()> {
    if SQLITE_POOL.get().is_some() {
        return Ok(());
    }
    ensure_sqlite_parent(&config.sqlite.path)?;
    let pool = build_sqlite_pool(
        SqliteConnectionConfig::new(&config.sqlite.path),
        config.pool.to_base_db(),
    )
    .map_err(|error| database_config_error(error.to_string()))?;
    SQLITE_POOL
        .set(pool)
        .map_err(|_| database_config_error("session SQLite连接池重复初始化"))
}

#[cfg(feature = "db-mysql")]
fn initialize_mysql_pool(config: &SessionDatabaseConfig) -> GlobalResult<()> {
    if MYSQL_POOL.get().is_some() {
        return Ok(());
    }
    let mysql = config
        .mysql
        .as_ref()
        .ok_or_else(|| database_config_error("db.mysql不能为空"))?;
    let password = mysql.password().map_err(database_config_error)?;
    let options = MySqlConnectOptions::new()
        .host(&mysql.host)
        .port(mysql.port)
        .database(&mysql.database)
        .pipes_as_concat(false)
        .username(&mysql.username)
        .password(&password)
        .ssl_mode(match mysql.ssl_mode {
            SessionMysqlSslMode::Disabled => MySqlSslMode::Disabled,
            SessionMysqlSslMode::Preferred => MySqlSslMode::Preferred,
            SessionMysqlSslMode::Required => MySqlSslMode::Required,
            SessionMysqlSslMode::VerifyCa => MySqlSslMode::VerifyCa,
            SessionMysqlSslMode::VerifyIdentity => MySqlSslMode::VerifyIdentity,
        });
    let options = apply_mysql_attributes(options, &mysql.attrs);
    let pool = build_mysql_pool(options, config.pool.to_base_db())
        .map_err(|error| database_config_error(error.to_string()))?;
    MYSQL_POOL
        .set(pool)
        .map_err(|_| database_config_error("session MySQL连接池重复初始化"))
}

#[cfg(feature = "db-mysql")]
fn apply_mysql_attributes(
    mut options: MySqlConnectOptions,
    attrs: &SessionMysqlAttrsConfig,
) -> MySqlConnectOptions {
    if let Some(level) = &attrs.log_global_sql_level {
        options = options.log_statements(base::logger::level_filter(level));
    }
    if let Some(timeout_sec) = attrs.log_slow_sql_timeout_sec {
        options = options.log_slow_statements(
            base::log::LevelFilter::Warn,
            Duration::from_secs(timeout_sec),
        );
    }
    if let Some(timezone) = &attrs.timezone {
        options = options.timezone(Some(timezone.clone()));
    }
    if let Some(charset) = &attrs.charset {
        options = options.charset(charset);
    }
    if let Some(path) = attrs
        .ssl_ca_crt_file
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        options = options.ssl_ca(path);
    }
    if let Some(path) = attrs
        .ssl_client_cert_file
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        options = options.ssl_client_cert(path);
    }
    if let Some(path) = attrs
        .ssl_client_key_file
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        options = options.ssl_client_key(path);
    }
    options
}

fn database_config_error(message: impl Into<String>) -> GlobalError {
    GlobalError::new_sys_error(
        &format!("session数据库配置无效: {}", message.into()),
        |msg| error!("{msg}"),
    )
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
    let config = SessionDatabaseConfig::get();
    match config.backend {
        #[cfg(feature = "db-mysql")]
        SessionDatabaseBackend::Mysql => {
            initialize_mysql_pool(&config)?;
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
            initialize_sqlite_pool(&config)?;
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
        ("terminated_at", "datetime(3) NULL"),
        ("terminal_reason", "varchar(64) NULL"),
        ("error_code", "varchar(64) NULL"),
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
    let history_index_exists: Option<i64> = base_db::sqlx::query_scalar(
        "SELECT 1 FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name='gb28181_sip_dialog_session' AND index_name='idx_gmv_sip_dialog_history' LIMIT 1",
    )
    .fetch_optional(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    if history_index_exists.is_none() {
        base_db::sqlx::query(
            "CREATE INDEX idx_gmv_sip_dialog_history ON gb28181_sip_dialog_session (signal_node_id, state, terminated_at DESC, stream_id DESC)",
        )
        .execute(mysql_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    }
    ensure_mysql_cloud_recording_columns().await?;
    Ok(())
}

#[cfg(feature = "db-mysql")]
async fn ensure_mysql_cloud_recording_columns() -> GlobalResult<()> {
    let existing: Vec<String> = base_db::sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='gb28181_record'",
    )
    .fetch_all(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    const COLUMNS: &[(&str, &str)] = &[
        ("request_id", "varchar(128) NULL"),
        ("session_node_id", "varchar(64) NULL"),
        ("stream_id", "varchar(64) NULL"),
        ("status", "varchar(16) NULL"),
        ("file_state", "varchar(16) NULL"),
        ("recorded_duration_ms", "bigint unsigned NOT NULL DEFAULT 0"),
        ("current_size_bytes", "bigint unsigned NOT NULL DEFAULT 0"),
        ("terminal_reason", "varchar(64) NULL"),
        ("error_code", "varchar(64) NULL"),
        ("error_message", "varchar(255) NULL"),
        ("started_at", "datetime NULL"),
        ("finished_at", "datetime NULL"),
        ("deleted_at", "datetime NULL"),
        ("version", "bigint unsigned NOT NULL DEFAULT 0"),
    ];
    for (name, definition) in COLUMNS {
        if !existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_record ADD COLUMN {name} {definition}"
            )))
            .execute(mysql_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    base_db::sqlx::query(
        "UPDATE gb28181_record SET status=CASE state WHEN 1 THEN 'COMPLETED' WHEN 2 THEN 'PARTIAL' WHEN 3 THEN 'FAILED' ELSE 'RUNNING' END WHERE status IS NULL",
    )
    .execute(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    base_db::sqlx::query(
        "UPDATE gb28181_record r SET file_state=CASE WHEN EXISTS(SELECT 1 FROM gb28181_file_info f WHERE f.biz_id=r.biz_id AND COALESCE(f.is_del,0)=0) THEN 'READY' WHEN r.status='RUNNING' THEN 'WRITING' ELSE 'NONE' END WHERE file_state IS NULL",
    )
    .execute(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    for (name, columns, unique) in [
        ("idx_gb28181_record_request_id", "request_id", true),
        (
            "idx_gb28181_record_channel_status",
            "device_id, channel_id, status, ct DESC",
            false,
        ),
        ("idx_gb28181_record_stream_id", "stream_id", false),
    ] {
        let found: Option<i64> = base_db::sqlx::query_scalar(
            "SELECT 1 FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name='gb28181_record' AND index_name=? LIMIT 1",
        )
        .bind(name)
        .fetch_optional(mysql_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
        if found.is_none() {
            let unique = if unique { "UNIQUE " } else { "" };
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "CREATE {unique}INDEX {name} ON gb28181_record ({columns})"
            )))
            .execute(mysql_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    let file_existing: Vec<String> = base_db::sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='gb28181_file_info'",
    )
    .fetch_all(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    for (name, definition) in [
        ("storage_id", "varchar(64) NULL"),
        ("file_state", "varchar(16) NULL"),
        ("duration_ms", "bigint unsigned NOT NULL DEFAULT 0"),
        ("mime_type", "varchar(64) NULL"),
    ] {
        if !file_existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_file_info ADD COLUMN {name} {definition}"
            )))
            .execute(mysql_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    let file_index: Option<i64> = base_db::sqlx::query_scalar(
        "SELECT 1 FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name='gb28181_file_info' AND index_name='idx_gb28181_file_biz_id' LIMIT 1",
    )
    .fetch_optional(mysql_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    if file_index.is_none() {
        base_db::sqlx::query(
            "CREATE INDEX idx_gb28181_file_biz_id ON gb28181_file_info (biz_id, is_del, id DESC)",
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
        ("terminated_at", "DATETIME NULL"),
        ("terminal_reason", "VARCHAR(64) NULL"),
        ("error_code", "VARCHAR(64) NULL"),
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
    base_db::sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_history ON gb28181_sip_dialog_session (signal_node_id, state, terminated_at DESC, stream_id DESC)",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    ensure_sqlite_cloud_recording_columns().await?;
    Ok(())
}

#[cfg(feature = "db-sqlite")]
async fn ensure_sqlite_cloud_recording_columns() -> GlobalResult<()> {
    use base_db::sqlx::Row;
    let rows = base_db::sqlx::query("PRAGMA table_info(gb28181_record)")
        .fetch_all(sqlite_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    let existing: Vec<String> = rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    const COLUMNS: &[(&str, &str)] = &[
        ("request_id", "VARCHAR(128) NULL"),
        ("session_node_id", "VARCHAR(64) NULL"),
        ("stream_id", "VARCHAR(64) NULL"),
        ("status", "VARCHAR(16) NULL"),
        ("file_state", "VARCHAR(16) NULL"),
        ("recorded_duration_ms", "BIGINT NOT NULL DEFAULT 0"),
        ("current_size_bytes", "BIGINT NOT NULL DEFAULT 0"),
        ("terminal_reason", "VARCHAR(64) NULL"),
        ("error_code", "VARCHAR(64) NULL"),
        ("error_message", "VARCHAR(255) NULL"),
        ("started_at", "DATETIME NULL"),
        ("finished_at", "DATETIME NULL"),
        ("deleted_at", "DATETIME NULL"),
        ("version", "BIGINT NOT NULL DEFAULT 0"),
    ];
    for (name, definition) in COLUMNS {
        if !existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_record ADD COLUMN {name} {definition}"
            )))
            .execute(sqlite_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    base_db::sqlx::query(
        "UPDATE gb28181_record SET status=CASE state WHEN 1 THEN 'COMPLETED' WHEN 2 THEN 'PARTIAL' WHEN 3 THEN 'FAILED' ELSE 'RUNNING' END WHERE status IS NULL",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    base_db::sqlx::query(
        "UPDATE gb28181_record SET file_state=CASE WHEN EXISTS(SELECT 1 FROM gb28181_file_info f WHERE f.biz_id=gb28181_record.biz_id AND COALESCE(f.is_del,0)=0) THEN 'READY' WHEN status='RUNNING' THEN 'WRITING' ELSE 'NONE' END WHERE file_state IS NULL",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    base_db::sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_gb28181_record_request_id ON gb28181_record (request_id)",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    base_db::sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gb28181_record_channel_status ON gb28181_record (device_id, channel_id, status, ct DESC)",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    base_db::sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gb28181_record_stream_id ON gb28181_record (stream_id)",
    )
    .execute(sqlite_pool())
    .await
    .hand_log(|msg| error!("{msg}"))?;
    let file_rows = base_db::sqlx::query("PRAGMA table_info(gb28181_file_info)")
        .fetch_all(sqlite_pool())
        .await
        .hand_log(|msg| error!("{msg}"))?;
    let file_existing: Vec<String> = file_rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    for (name, definition) in [
        ("storage_id", "VARCHAR(64) NULL"),
        ("file_state", "VARCHAR(16) NULL"),
        ("duration_ms", "BIGINT NOT NULL DEFAULT 0"),
        ("mime_type", "VARCHAR(64) NULL"),
    ] {
        if !file_existing.iter().any(|column| column == name) {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE gb28181_file_info ADD COLUMN {name} {definition}"
            )))
            .execute(sqlite_pool())
            .await
            .hand_log(|msg| error!("{msg}"))?;
        }
    }
    base_db::sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gb28181_file_biz_id ON gb28181_file_info (biz_id, is_del, id DESC)",
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
    fn database_pool_rejects_invalid_limits_and_timeout() {
        let invalid_limits = SessionPoolConfig {
            max_connections: 1,
            min_connections: 2,
            ..SessionPoolConfig::default()
        };
        assert!(invalid_limits.validate().is_err());

        let invalid_timeout = SessionPoolConfig {
            connection_timeout_sec: 0,
            ..SessionPoolConfig::default()
        };
        assert!(invalid_timeout.validate().is_err());
    }

    #[test]
    fn mysql_client_certificate_requires_private_key() {
        let attrs = SessionMysqlAttrsConfig {
            ssl_client_cert_file: Some(PathBuf::from("client.crt")),
            ..SessionMysqlAttrsConfig::default()
        };

        assert!(attrs.validate().is_err());
    }

    #[test]
    fn example_database_config_deserializes_and_validates() {
        let yaml: base::serde_yaml::Value =
            base::serde_yaml::from_str(include_str!("../../config.yml")).unwrap();
        let database = yaml.get("db").cloned().unwrap();
        let config: SessionDatabaseConfig = base::serde_yaml::from_value(database).unwrap();

        config.pool.validate().unwrap();
        config.mysql.as_ref().unwrap().validate().unwrap();
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

    #[cfg(all(feature = "db-mysql", feature = "db-sqlite"))]
    #[test]
    fn dialog_schema_types_match_rust_model_on_both_backends() {
        let mysql_start = MYSQL_SCHEMA
            .find("CREATE TABLE IF NOT EXISTS `gb28181_sip_dialog_session`")
            .expect("MySQL dialog table");
        let mysql_tail = &MYSQL_SCHEMA[mysql_start..];
        let mysql_end = mysql_tail.find(") ENGINE").expect("MySQL dialog end");
        let mysql = mysql_tail[..mysql_end].to_ascii_lowercase();
        let sqlite_start = SQLITE_SCHEMA
            .find("CREATE TABLE IF NOT EXISTS gb28181_sip_dialog_session")
            .expect("SQLite dialog table");
        let sqlite_tail = &SQLITE_SCHEMA[sqlite_start..];
        let sqlite_end = sqlite_tail.find("\n);").expect("SQLite dialog end");
        let sqlite = sqlite_tail[..sqlite_end].to_ascii_lowercase();

        for column in [
            "stream_id",
            "device_id",
            "channel_id",
            "session_type",
            "signal_node_id",
            "media_node_id",
            "call_id",
            "local_uri",
            "remote_uri",
            "local_tag",
            "local_sip_addr",
            "remote_sip_addr",
            "transport",
            "state",
        ] {
            assert!(mysql.contains(&format!("`{column}` varchar")));
            assert!(sqlite.contains(&format!("{column} varchar")));
        }
        for column in [
            "ssrc",
            "registration_epoch_id",
            "remote_tag",
            "contact_uri",
            "terminal_reason",
            "error_code",
        ] {
            assert!(mysql.contains(&format!("`{column}` varchar")));
            assert!(sqlite.contains(&format!("{column} varchar")));
        }
        for column in ["local_cseq", "remote_cseq", "version"] {
            assert!(mysql.contains(&format!("`{column}` bigint")));
            assert!(sqlite.contains(&format!("{column} bigint")));
        }
        for column in [
            "established_at",
            "terminated_at",
            "last_seen_at",
            "expire_at",
            "created_at",
            "updated_at",
        ] {
            assert!(mysql.contains(&format!("`{column}` datetime")));
            assert!(sqlite.contains(&format!("{column} datetime")));
        }
        assert!(mysql.contains("`route_set` text"));
        assert!(sqlite.contains("route_set text"));
        assert!(
            !mysql.contains("bigint unsigned"),
            "SipDialogSessionRow uses i64 and must not decode MySQL BIGINT UNSIGNED"
        );
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

            let dialog_columns = base_db::sqlx::query(
                "SELECT name FROM pragma_table_info('gb28181_sip_dialog_session')",
            )
            .fetch_all(&pool)
            .await
            .expect("read dialog columns")
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
            for column in ["terminated_at", "terminal_reason", "error_code"] {
                assert!(dialog_columns.iter().any(|item| item == column));
            }

            let history_index: i64 = base_db::sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_gb28181_sip_dialog_history'",
            )
            .fetch_one(&pool)
            .await
            .expect("read dialog history index");
            assert_eq!(history_index, 1);

            let now = base::chrono::Local::now().naive_local();
            base_db::sqlx::query(
                "INSERT INTO gb28181_sip_dialog_session (stream_id,device_id,channel_id,session_type,signal_node_id,media_node_id,ssrc,call_id,local_uri,remote_uri,local_tag,local_cseq,local_sip_addr,remote_sip_addr,transport,state,terminated_at,terminal_reason,last_seen_at,expire_at,version,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind("schema-type-stream")
            .bind("34020000001320000001")
            .bind("34020000001320000002")
            .bind("LIVE")
            .bind("34020000002000000001")
            .bind("stream-1")
            .bind("0100000001")
            .bind("schema-type-call")
            .bind("sip:local@example.test")
            .bind("sip:remote@example.test")
            .bind("local-tag")
            .bind(1_i64)
            .bind("127.0.0.1:5060")
            .bind("127.0.0.1:5061")
            .bind("UDP")
            .bind("TERMINATED")
            .bind(now)
            .bind("session_close")
            .bind(now)
            .bind(now + base::chrono::Duration::hours(8))
            .bind(0_i64)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .expect("insert dialog type contract row");
            let row = base_db::sqlx::query_as::<
                _,
                (String, Option<String>, Option<base::chrono::NaiveDateTime>, i64, base::chrono::NaiveDateTime),
            >(
                "SELECT stream_id,terminal_reason,terminated_at,local_cseq,created_at FROM gb28181_sip_dialog_session WHERE stream_id=?",
            )
                .bind("schema-type-stream")
                .fetch_one(&pool)
                .await
                .expect("decode dialog type contract row");
            assert_eq!(row.0, "schema-type-stream");
            assert_eq!(row.1.as_deref(), Some("session_close"));
            assert_eq!(row.2, Some(now));
            assert_eq!(row.3, 1);
            assert_eq!(row.4, now);

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
