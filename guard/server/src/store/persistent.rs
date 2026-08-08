#[cfg(feature = "db-sqlite")]
use std::path::Path;

#[cfg(feature = "db-mysql")]
use base_db::dbx::mysqlx::build_mysql_pool;
#[cfg(feature = "db-sqlite")]
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
#[cfg(feature = "db-mysql")]
use base_db::sqlx::ConnectOptions;
#[cfg(feature = "db-mysql")]
use base_db::sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

use crate::app_config::{DatabaseBackend, GuardAppConfig};
#[cfg(feature = "db-mysql")]
use crate::app_config::{MysqlAttrsConfig, MysqlSslMode as ConfigSslMode};
use crate::auth::{Role, UserAccess, UserAccount, UserProfile};
use crate::core::{GuardError, GuardResult};
use crate::integration::model::{
    Integration, IntegrationAudit, IntegrationCredential, IntegrationHttpConfig,
    IntegrationMapping, IntegrationMqttConfig,
};
use crate::outbox::OutboxRepository;
use crate::store::command::HttpCommandClaim;
#[cfg(feature = "db-mysql")]
use crate::store::mysql::MysqlStore;
#[cfg(feature = "db-sqlite")]
use crate::store::sqlite::SqliteStore;

#[derive(Debug, Clone)]
pub enum UserRepository {
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

#[derive(Debug, Clone)]
pub enum IntegrationRepository {
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

#[derive(Debug, Clone)]
pub enum CommandRepository {
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

impl CommandRepository {
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_http(
        &self,
        command_id: &str,
        integration_id: &str,
        operation_id: &str,
        action: &str,
        request_hash: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> GuardResult<HttpCommandClaim> {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => {
                store
                    .claim_http_command(
                        command_id,
                        integration_id,
                        operation_id,
                        action,
                        request_hash,
                        expires_at_ms,
                        now_ms,
                    )
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => {
                store
                    .claim_http_command(
                        command_id,
                        integration_id,
                        operation_id,
                        action,
                        request_hash,
                        expires_at_ms,
                        now_ms,
                    )
                    .await
            }
        }
    }

    pub async fn complete_http(
        &self,
        command_id: &str,
        status: u16,
        response_body: &[u8],
        now_ms: i64,
    ) -> GuardResult<()> {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => {
                store
                    .complete_http_command(command_id, status, response_body, now_ms)
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => {
                store
                    .complete_http_command(command_id, status, response_body, now_ms)
                    .await
            }
        }
    }
}

macro_rules! dispatch_integration {
    ($self:expr, $method:ident($($argument:expr),* $(,)?)) => {
        match $self {
            #[cfg(feature = "db-mysql")]
            IntegrationRepository::Mysql(store) => store.$method($($argument),*).await,
            #[cfg(feature = "db-sqlite")]
            IntegrationRepository::Sqlite(store) => store.$method($($argument),*).await,
        }
    };
}

impl IntegrationRepository {
    pub async fn list(&self) -> GuardResult<Vec<Integration>> {
        dispatch_integration!(self, list_integrations())
    }

    pub async fn get(&self, integration_id: &str) -> GuardResult<Option<Integration>> {
        dispatch_integration!(self, get_integration(integration_id))
    }

    pub async fn upsert(&self, value: &Integration) -> GuardResult<()> {
        dispatch_integration!(self, upsert_integration(value))
    }

    pub async fn list_credentials(
        &self,
        integration_id: &str,
    ) -> GuardResult<Vec<IntegrationCredential>> {
        dispatch_integration!(self, list_integration_credentials(integration_id))
    }

    pub async fn find_credential(
        &self,
        access_key: &str,
    ) -> GuardResult<Option<IntegrationCredential>> {
        dispatch_integration!(self, find_integration_credential(access_key))
    }

    pub async fn insert_credential(&self, value: &IntegrationCredential) -> GuardResult<()> {
        dispatch_integration!(self, insert_integration_credential(value))
    }

    pub async fn revoke_credential(&self, credential_id: &str, now_ms: i64) -> GuardResult<()> {
        dispatch_integration!(self, revoke_integration_credential(credential_id, now_ms))
    }

    pub async fn http_config(
        &self,
        integration_id: &str,
    ) -> GuardResult<Option<IntegrationHttpConfig>> {
        dispatch_integration!(self, get_integration_http_config(integration_id))
    }

    pub async fn upsert_http_config(&self, value: &IntegrationHttpConfig) -> GuardResult<()> {
        dispatch_integration!(self, upsert_integration_http_config(value))
    }

    pub async fn mqtt_config(
        &self,
        integration_id: &str,
    ) -> GuardResult<Option<IntegrationMqttConfig>> {
        dispatch_integration!(self, get_integration_mqtt_config(integration_id))
    }

    pub async fn upsert_mqtt_config(&self, value: &IntegrationMqttConfig) -> GuardResult<()> {
        dispatch_integration!(self, upsert_integration_mqtt_config(value))
    }

    pub async fn authorize_mqtt_command(
        &self,
        topic: &str,
        integration_id: &str,
        action: &str,
        protocol_version: &str,
        now_ms: i64,
    ) -> GuardResult<IntegrationMqttConfig> {
        let integration = self
            .get(integration_id)
            .await?
            .filter(|integration| {
                integration.transport == crate::integration::model::IntegrationTransport::Mqtt
                    && integration.inbound_enabled
                    && integration.enabled
                    && integration
                        .expires_at_ms
                        .is_none_or(|expires_at| expires_at > now_ms)
            })
            .ok_or_else(|| {
                GuardError::InvalidIdentity("MQTT integration is not active".to_string())
            })?;
        let config = self
            .mqtt_config(&integration.integration_id)
            .await?
            .ok_or_else(|| {
                GuardError::InvalidIdentity("MQTT integration config is missing".to_string())
            })?;
        if config.protocol_version != protocol_version {
            return Err(GuardError::InvalidIdentity(
                "MQTT integration protocol does not match runtime".to_string(),
            ));
        }
        if config.command_topic != topic {
            return Err(GuardError::InvalidIdentity(
                "MQTT command topic is not authorized".to_string(),
            ));
        }
        if !config
            .allowed_actions
            .iter()
            .any(|allowed| allowed == action)
        {
            return Err(GuardError::InvalidIdentity(
                "MQTT command action is not allowed".to_string(),
            ));
        }
        Ok(config)
    }

    pub async fn list_mappings(
        &self,
        integration_id: &str,
    ) -> GuardResult<Vec<IntegrationMapping>> {
        dispatch_integration!(self, list_integration_mappings(integration_id))
    }

    pub async fn upsert_mapping(&self, value: &IntegrationMapping) -> GuardResult<()> {
        dispatch_integration!(self, upsert_integration_mapping(value))
    }

    pub async fn append_audit(&self, value: &IntegrationAudit) -> GuardResult<()> {
        dispatch_integration!(self, append_integration_audit(value))
    }

    pub async fn list_audits(&self, limit: usize) -> GuardResult<Vec<IntegrationAudit>> {
        dispatch_integration!(self, list_integration_audits(limit))
    }
}

impl UserRepository {
    pub async fn list_profiles(&self) -> GuardResult<Vec<UserProfile>> {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.list_user_profiles().await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.list_user_profiles().await,
        }
    }

    pub async fn load_user(&self, username: &str) -> GuardResult<Option<UserAccount>> {
        let user = match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.load_user(username).await?,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.load_user(username).await?,
        };
        if let Some(user) = &user {
            user.validate_password_hash()?;
        }
        Ok(user)
    }

    pub async fn upsert_user(
        &self,
        username: &str,
        role: Role,
        password_hash: Option<&str>,
        nickname: Option<&str>,
        access: UserAccess,
        now_ms: i64,
    ) -> GuardResult<()> {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => {
                store
                    .upsert_user(username, role, password_hash, nickname, access, now_ms)
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => {
                store
                    .upsert_user(username, role, password_hash, nickname, access, now_ms)
                    .await
            }
        }
    }
}

pub enum PersistentStore {
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

impl PersistentStore {
    pub async fn connect(config: &GuardAppConfig) -> GuardResult<Self> {
        match config.database.backend {
            #[cfg(feature = "db-sqlite")]
            DatabaseBackend::Sqlite => {
                ensure_parent(&config.database.sqlite.path)?;
                let pool = build_sqlite_pool(
                    SqliteConnectionConfig::new(&config.database.sqlite.path),
                    config.database.pool.to_base_db(),
                )
                .map_err(database_error)?;
                Ok(Self::Sqlite(SqliteStore::new(pool)))
            }
            #[cfg(not(feature = "db-sqlite"))]
            DatabaseBackend::Sqlite => Err(database_backend_not_enabled("sqlite")),
            #[cfg(feature = "db-mysql")]
            DatabaseBackend::Mysql => {
                let mysql = config.database.mysql.as_ref().ok_or_else(|| {
                    GuardError::InvalidConfig("guard.database.mysql is required".to_string())
                })?;
                let password = mysql.password()?;
                let options = MySqlConnectOptions::new()
                    .host(&mysql.host)
                    .port(mysql.port)
                    .database(&mysql.database)
                    .username(&mysql.username)
                    .password(&password)
                    .ssl_mode(match mysql.ssl_mode {
                        ConfigSslMode::Disabled => MySqlSslMode::Disabled,
                        ConfigSslMode::Preferred => MySqlSslMode::Preferred,
                        ConfigSslMode::Required => MySqlSslMode::Required,
                        ConfigSslMode::VerifyCa => MySqlSslMode::VerifyCa,
                        ConfigSslMode::VerifyIdentity => MySqlSslMode::VerifyIdentity,
                    });
                let options = apply_mysql_attributes(options, &mysql.attrs);
                let pool = build_mysql_pool(options, config.database.pool.to_base_db())
                    .map_err(database_error)?;
                Ok(Self::Mysql(MysqlStore::new(pool)))
            }
            #[cfg(not(feature = "db-mysql"))]
            DatabaseBackend::Mysql => Err(database_backend_not_enabled("mysql")),
        }
    }

    pub async fn initialize(&self, config: &GuardAppConfig) -> GuardResult<()> {
        if config.database.auto_migrate {
            self.migrate().await?;
        }
        if self.load_users().await?.is_empty() {
            let hash = config.bootstrap.admin.password_hash()?;
            UserAccount::new(&config.bootstrap.admin.username, Role::Admin, &hash)
                .validate_password_hash()?;
            match self {
                #[cfg(feature = "db-mysql")]
                Self::Mysql(store) => {
                    store
                        .bootstrap_admin(&config.bootstrap.admin.username, &hash)
                        .await?;
                }
                #[cfg(feature = "db-sqlite")]
                Self::Sqlite(store) => {
                    store
                        .bootstrap_admin(&config.bootstrap.admin.username, &hash)
                        .await?;
                }
            }
        }
        if self.load_users().await?.is_empty() {
            return Err(GuardError::InvalidConfig(
                "guard_user is empty; enable bootstrap admin or run manual initialization SQL"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub async fn migrate(&self) -> GuardResult<()> {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.migrate().await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.migrate().await,
        }
    }

    pub async fn load_users(&self) -> GuardResult<Vec<UserAccount>> {
        let users = match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.load_users().await?,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.load_users().await?,
        };
        for user in &users {
            user.validate_password_hash()?;
        }
        Ok(users)
    }

    pub fn user_repository(&self) -> UserRepository {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => UserRepository::Mysql(store.clone()),
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => UserRepository::Sqlite(store.clone()),
        }
    }

    pub fn integration_repository(&self) -> IntegrationRepository {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => IntegrationRepository::Mysql(store.clone()),
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => IntegrationRepository::Sqlite(store.clone()),
        }
    }

    pub fn command_repository(&self) -> CommandRepository {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => CommandRepository::Mysql(store.clone()),
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => CommandRepository::Sqlite(store.clone()),
        }
    }

    pub fn outbox_repository(&self) -> OutboxRepository {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => OutboxRepository::from(store.clone()),
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => OutboxRepository::from(store.clone()),
        }
    }

    pub async fn close(&self) {
        match self {
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.pool().close().await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.pool().close().await,
        }
    }
}

#[cfg(feature = "db-mysql")]
fn apply_mysql_attributes(
    mut options: MySqlConnectOptions,
    attrs: &MysqlAttrsConfig,
) -> MySqlConnectOptions {
    if let Some(level) = &attrs.log_global_sql_level {
        options = options.log_statements(base::logger::level_filter(level));
    }
    if let Some(timeout_sec) = attrs.log_slow_sql_timeout_sec {
        options = options.log_slow_statements(
            base::log::LevelFilter::Warn,
            std::time::Duration::from_secs(timeout_sec),
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

#[cfg(feature = "db-sqlite")]
fn ensure_parent(path: &Path) -> GuardResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            GuardError::InvalidConfig(format!("create SQLite directory failed: {error}"))
        })?;
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> GuardError {
    GuardError::Conflict(format!("database initialization failed: {error}"))
}

#[cfg(all(test, feature = "db-sqlite"))]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn ensure_parent_creates_missing_sqlite_directory() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("gmv-guard-sqlite-parent")
            .join(unique.to_string())
            .join("nested");
        let path = dir.join("guard.db");

        ensure_parent(&path).expect("create sqlite parent");

        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir()
                .join("gmv-guard-sqlite-parent")
                .join(unique.to_string()),
        );
    }
}

#[cfg(not(all(feature = "db-mysql", feature = "db-sqlite")))]
fn database_backend_not_enabled(backend: &str) -> GuardError {
    GuardError::InvalidConfig(format!(
        "guard database backend {backend} is not enabled in this binary"
    ))
}
