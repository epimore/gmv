use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;

use crate::core::{GuardError, GuardResult};

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard", check)]
pub struct GuardAppConfig {
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub grpc: GrpcConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
    #[serde(default)]
    pub simulator: SimulatorConfig,
    #[serde(default)]
    pub integrations: IntegrationsConfig,
}

impl GuardAppConfig {
    pub fn load(path: impl Into<String>) -> Self {
        base::cfg_lib::conf::init_cfg(path.into());
        Self::conf()
    }

    pub fn validate(&self) -> GuardResult<()> {
        self.http.validate()?;
        self.grpc.validate()?;
        self.database.validate()?;
        self.security.validate()?;
        self.bootstrap.validate()?;
        self.integrations.validate()
    }
}

impl CheckFromConf for GuardAppConfig {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        self.validate()
            .map_err(|error| FieldCheckError::BizError(error.to_string()))
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct GrpcConfig {
    #[serde(default = "default_grpc_bind_addr")]
    pub bind_addr: SocketAddr,
    #[serde(default = "default_heartbeat_interval_ms")]
    pub heartbeat_interval_ms: u64,
    #[serde(default = "default_heartbeat_timeout_ms")]
    pub heartbeat_timeout_ms: u64,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_grpc_bind_addr(),
            heartbeat_interval_ms: default_heartbeat_interval_ms(),
            heartbeat_timeout_ms: default_heartbeat_timeout_ms(),
        }
    }
}

impl GrpcConfig {
    fn validate(&self) -> GuardResult<()> {
        if !self.bind_addr.ip().is_loopback() {
            return Err(GuardError::InvalidConfig(
                "guard.grpc currently requires a loopback bind until RPC TLS is configured"
                    .to_string(),
            ));
        }
        if self.heartbeat_interval_ms == 0
            || self.heartbeat_timeout_ms < self.heartbeat_interval_ms.saturating_mul(3)
        {
            return Err(GuardError::InvalidConfig(
                "guard.grpc heartbeat timeout must cover at least three intervals".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct HttpConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: SocketAddr,
    #[serde(default = "default_origin")]
    pub allowed_origin: String,
    #[serde(default = "default_ui_dist_dir")]
    pub ui_dist_dir: PathBuf,
    #[serde(default)]
    pub tls: HttpTlsConfig,
    #[serde(default = "default_session_ttl_sec")]
    pub session_ttl_sec: u64,
    #[serde(default = "default_login_window_sec")]
    pub login_window_sec: u64,
    #[serde(default = "default_max_failed_attempts")]
    pub max_failed_attempts: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            allowed_origin: default_origin(),
            ui_dist_dir: default_ui_dist_dir(),
            tls: HttpTlsConfig::default(),
            session_ttl_sec: default_session_ttl_sec(),
            login_window_sec: default_login_window_sec(),
            max_failed_attempts: default_max_failed_attempts(),
        }
    }
}

impl HttpConfig {
    fn validate(&self) -> GuardResult<()> {
        if !self.tls.enabled && !self.bind_addr.ip().is_loopback() {
            return Err(GuardError::InvalidConfig(
                "TLS can only be disabled on a loopback HTTP bind".to_string(),
            ));
        }
        if self.tls.enabled
            && (self.tls.certificate_path.as_os_str().is_empty()
                || self.tls.private_key_path.as_os_str().is_empty())
        {
            return Err(GuardError::InvalidConfig(
                "guard.http.tls certificate_path and private_key_path are required".to_string(),
            ));
        }
        if self
            .allowed_origin
            .parse::<axum::http::HeaderValue>()
            .is_err()
        {
            return Err(GuardError::InvalidConfig(
                "guard.http.allowed_origin is invalid".to_string(),
            ));
        }
        if self.session_ttl_sec == 0 || self.login_window_sec == 0 || self.max_failed_attempts == 0
        {
            return Err(GuardError::InvalidConfig(
                "guard.http session and login limits must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct HttpTlsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}

impl Default for HttpTlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            certificate_path: PathBuf::new(),
            private_key_path: PathBuf::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(crate = "base::serde", rename_all = "lowercase")]
pub enum DatabaseBackend {
    Sqlite,
    Mysql,
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct DatabaseConfig {
    #[serde(default = "default_database_backend")]
    pub backend: DatabaseBackend,
    #[serde(default = "default_true")]
    pub auto_migrate: bool,
    #[serde(default)]
    pub pool: PoolConfig,
    #[serde(default)]
    pub sqlite: SqliteConfig,
    pub mysql: Option<MysqlConfig>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            backend: DatabaseBackend::Sqlite,
            auto_migrate: true,
            pool: PoolConfig::default(),
            sqlite: SqliteConfig::default(),
            mysql: None,
        }
    }
}

impl DatabaseConfig {
    fn validate(&self) -> GuardResult<()> {
        self.pool.validate()?;
        match self.backend {
            DatabaseBackend::Sqlite if self.sqlite.path.as_os_str().is_empty() => Err(
                GuardError::InvalidConfig("guard.database.sqlite.path is required".to_string()),
            ),
            DatabaseBackend::Mysql => self
                .mysql
                .as_ref()
                .ok_or_else(|| {
                    GuardError::InvalidConfig("guard.database.mysql is required".to_string())
                })?
                .validate(),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct PoolConfig {
    #[serde(default = "default_pool_max")]
    pub max_connections: u32,
    #[serde(default)]
    pub min_connections: u32,
    #[serde(default = "default_pool_timeout_sec")]
    pub connection_timeout_sec: u64,
    #[serde(default = "default_pool_lifetime_sec")]
    pub max_lifetime_sec: u64,
    #[serde(default = "default_pool_idle_sec")]
    pub idle_timeout_sec: u64,
    #[serde(default)]
    pub check_health: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: default_pool_max(),
            min_connections: 0,
            connection_timeout_sec: default_pool_timeout_sec(),
            max_lifetime_sec: default_pool_lifetime_sec(),
            idle_timeout_sec: default_pool_idle_sec(),
            check_health: false,
        }
    }
}

impl PoolConfig {
    fn validate(&self) -> GuardResult<()> {
        if self.max_connections == 0 || self.min_connections > self.max_connections {
            return Err(GuardError::InvalidConfig(
                "guard.database.pool connection limits are invalid".to_string(),
            ));
        }
        Ok(())
    }

    pub fn to_base_db(&self) -> base_db::dbx::DatabasePoolConfig {
        base_db::dbx::DatabasePoolConfig {
            max_size: self.max_connections,
            min_idle: Some(self.min_connections),
            connection_timeout: Duration::from_secs(self.connection_timeout_sec),
            max_lifetime: Some(Duration::from_secs(self.max_lifetime_sec)),
            idle_timeout: Some(Duration::from_secs(self.idle_timeout_sec)),
            test_on_check_out: self.check_health,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SqliteConfig {
    #[serde(default = "default_sqlite_path")]
    pub path: PathBuf,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: default_sqlite_path(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct MysqlConfig {
    pub host: String,
    #[serde(default = "default_mysql_port")]
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password_env: String,
    #[serde(default)]
    pub ssl_mode: MysqlSslMode,
}

impl MysqlConfig {
    fn validate(&self) -> GuardResult<()> {
        if self.host.trim().is_empty()
            || self.database.trim().is_empty()
            || self.username.trim().is_empty()
            || self.password_env.trim().is_empty()
        {
            return Err(GuardError::InvalidConfig(
                "guard.database.mysql connection fields are required".to_string(),
            ));
        }
        Ok(())
    }

    pub fn password(&self) -> GuardResult<String> {
        required_env(&self.password_env)
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum MysqlSslMode {
    Disabled,
    #[default]
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SecurityConfig {
    #[serde(default = "default_master_key_env")]
    pub master_key_env: String,
    #[serde(default)]
    pub persist_ui_sessions: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            master_key_env: default_master_key_env(),
            persist_ui_sessions: false,
        }
    }
}

impl SecurityConfig {
    fn validate(&self) -> GuardResult<()> {
        if self.master_key_env.trim().is_empty() {
            return Err(GuardError::InvalidConfig(
                "guard.security.master_key_env is required".to_string(),
            ));
        }
        Ok(())
    }

    pub fn master_key(&self) -> GuardResult<String> {
        required_env(&self.master_key_env)
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct BootstrapConfig {
    #[serde(default)]
    pub admin: BootstrapAdminConfig,
}

impl BootstrapConfig {
    fn validate(&self) -> GuardResult<()> {
        self.admin.validate()
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct BootstrapAdminConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_admin_username")]
    pub username: String,
    #[serde(default = "default_admin_hash_env")]
    pub password_hash_env: String,
}

impl Default for BootstrapAdminConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            username: default_admin_username(),
            password_hash_env: default_admin_hash_env(),
        }
    }
}

impl BootstrapAdminConfig {
    fn validate(&self) -> GuardResult<()> {
        if self.enabled
            && (self.username.trim().is_empty() || self.password_hash_env.trim().is_empty())
        {
            return Err(GuardError::InvalidConfig(
                "guard.bootstrap.admin username and password_hash_env are required".to_string(),
            ));
        }
        Ok(())
    }

    pub fn password_hash(&self) -> GuardResult<String> {
        required_env(&self.password_hash_env)
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct SimulatorConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationsConfig {
    #[serde(default)]
    pub mqtt: MqttStartupConfig,
}

impl IntegrationsConfig {
    fn validate(&self) -> GuardResult<()> {
        self.mqtt.validate()
    }
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct MqttStartupConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub broker: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password_env: String,
    #[serde(default = "default_true")]
    pub tls: bool,
    #[serde(default)]
    pub subscribe_topics: Vec<String>,
}

impl Default for MqttStartupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker: String::new(),
            port: default_mqtt_port(),
            client_id: String::new(),
            username: String::new(),
            password_env: String::new(),
            tls: true,
            subscribe_topics: Vec::new(),
        }
    }
}

impl MqttStartupConfig {
    fn validate(&self) -> GuardResult<()> {
        if self.enabled
            && (self.broker.trim().is_empty()
                || self.client_id.trim().is_empty()
                || self.username.trim().is_empty()
                || self.password_env.trim().is_empty())
        {
            return Err(GuardError::InvalidConfig(
                "guard.integrations.mqtt connection fields are required when enabled".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn config_path_from_args() -> GuardResult<String> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|value| value == "start") {
        args.remove(0);
    }
    if args.is_empty() {
        return Ok("./config.yml".to_string());
    }
    if args.len() == 2 && matches!(args[0].as_str(), "-c" | "--config") {
        return Ok(args.remove(1));
    }
    Err(GuardError::InvalidConfig(
        "usage: guard [start] [-c|--config <path>]".to_string(),
    ))
}

pub fn required_env(name: &str) -> GuardResult<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            GuardError::InvalidConfig(format!("environment variable {name} is required"))
        })
}

fn default_true() -> bool {
    true
}
fn default_grpc_bind_addr() -> SocketAddr {
    "127.0.0.1:18080".parse().expect("valid default gRPC bind")
}

fn default_heartbeat_interval_ms() -> u64 {
    5_000
}
fn default_heartbeat_timeout_ms() -> u64 {
    20_000
}

fn default_bind_addr() -> SocketAddr {
    "127.0.0.1:8080".parse().unwrap()
}
fn default_origin() -> String {
    "https://127.0.0.1:8080".to_string()
}
fn default_ui_dist_dir() -> PathBuf {
    PathBuf::from("guard-ui/dist")
}
fn default_session_ttl_sec() -> u64 {
    8 * 60 * 60
}
fn default_login_window_sec() -> u64 {
    60
}
fn default_max_failed_attempts() -> usize {
    5
}
fn default_database_backend() -> DatabaseBackend {
    DatabaseBackend::Sqlite
}
fn default_sqlite_path() -> PathBuf {
    PathBuf::from("data/guard.db")
}
fn default_pool_max() -> u32 {
    10
}
fn default_pool_timeout_sec() -> u64 {
    8
}
fn default_pool_lifetime_sec() -> u64 {
    1800
}
fn default_pool_idle_sec() -> u64 {
    60
}
fn default_mysql_port() -> u16 {
    3306
}
fn default_master_key_env() -> String {
    "GMV_GUARD_MASTER_KEY".to_string()
}
fn default_admin_username() -> String {
    "admin".to_string()
}
fn default_admin_hash_env() -> String {
    "GMV_ADMIN_PASSWORD_HASH".to_string()
}
fn default_mqtt_port() -> u16 {
    8883
}
