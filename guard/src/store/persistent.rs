use std::path::Path;

use base::utils::crypto::Aes256GcmCipher;
use base_db::dbx::mysqlx::build_mysql_pool;
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
use base_db::sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

use crate::app_config::{DatabaseBackend, GuardAppConfig, MysqlSslMode as ConfigSslMode};
use crate::auth::{Role, UserAccount, UserProfile};
use crate::core::{GuardError, GuardResult};
use crate::outbox::OutboxRepository;
use crate::store::{mysql::MysqlStore, sqlite::SqliteStore};

#[derive(Debug, Clone)]
pub enum UserRepository {
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl UserRepository {
    pub async fn list_profiles(&self) -> GuardResult<Vec<UserProfile>> {
        match self {
            Self::Mysql(store) => store.list_user_profiles().await,
            Self::Sqlite(store) => store.list_user_profiles().await,
        }
    }

    pub async fn load_user(&self, username: &str) -> GuardResult<Option<UserAccount>> {
        let user = match self {
            Self::Mysql(store) => store.load_user(username).await?,
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
        enabled: bool,
        now_ms: i64,
    ) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => {
                store
                    .upsert_user(username, role, password_hash, nickname, enabled, now_ms)
                    .await
            }
            Self::Sqlite(store) => {
                store
                    .upsert_user(username, role, password_hash, nickname, enabled, now_ms)
                    .await
            }
        }
    }
}

pub enum PersistentStore {
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl PersistentStore {
    pub async fn connect(config: &GuardAppConfig) -> GuardResult<Self> {
        Aes256GcmCipher::from_base64_key_no_pad(&config.security.master_key()?)
            .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
        match config.database.backend {
            DatabaseBackend::Sqlite => {
                ensure_parent(&config.database.sqlite.path)?;
                let pool = build_sqlite_pool(
                    SqliteConnectionConfig::new(&config.database.sqlite.path),
                    config.database.pool.to_base_db(),
                )
                .map_err(database_error)?;
                Ok(Self::Sqlite(SqliteStore::new(pool)))
            }
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
                let pool = build_mysql_pool(options, config.database.pool.to_base_db())
                    .map_err(database_error)?;
                Ok(Self::Mysql(MysqlStore::new(pool)))
            }
        }
    }

    pub async fn initialize(&self, config: &GuardAppConfig) -> GuardResult<()> {
        if config.database.auto_migrate {
            self.migrate().await?;
        }
        if config.bootstrap.admin.enabled {
            let hash = config.bootstrap.admin.password_hash()?;
            UserAccount::new(&config.bootstrap.admin.username, Role::Admin, &hash)
                .validate_password_hash()?;
            match self {
                Self::Mysql(store) => {
                    store
                        .bootstrap_admin(&config.bootstrap.admin.username, &hash)
                        .await?;
                }
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
            Self::Mysql(store) => store.migrate().await,
            Self::Sqlite(store) => store.migrate().await,
        }
    }

    pub async fn load_users(&self) -> GuardResult<Vec<UserAccount>> {
        let users = match self {
            Self::Mysql(store) => store.load_users().await?,
            Self::Sqlite(store) => store.load_users().await?,
        };
        for user in &users {
            user.validate_password_hash()?;
        }
        Ok(users)
    }

    pub fn user_repository(&self) -> UserRepository {
        match self {
            Self::Mysql(store) => UserRepository::Mysql(store.clone()),
            Self::Sqlite(store) => UserRepository::Sqlite(store.clone()),
        }
    }

    pub fn outbox_repository(&self) -> OutboxRepository {
        match self {
            Self::Mysql(store) => OutboxRepository::from(store.clone()),
            Self::Sqlite(store) => OutboxRepository::from(store.clone()),
        }
    }
}

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
