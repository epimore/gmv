use std::path::Path;

use base_db::dbx::mysqlx::build_mysql_pool;
use base_db::dbx::sqlitex::{SqliteConnectionConfig, build_sqlite_pool};
use base_db::sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

use crate::app_config::{DatabaseBackend, GuardAppConfig, MysqlSslMode as ConfigSslMode};
use crate::auth::{Role, UserAccount, UserProfile};
use crate::core::{GuardError, GuardResult};
use crate::outbox::OutboxRepository;
use crate::store::{
    model::{
        GbChannelImageRecord, GbChannelRecord, GbDeviceRecord, GmvRecordInsert, MediaFileInsert,
        RecordFileInsert,
    },
    mysql::MysqlStore,
    sqlite::SqliteStore,
};

#[derive(Debug, Clone)]
pub enum GbRepository {
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl GbRepository {
    pub async fn list_devices(&self) -> GuardResult<Vec<GbDeviceRecord>> {
        match self {
            Self::Mysql(store) => store.list_gb_devices().await,
            Self::Sqlite(store) => store.list_gb_devices().await,
        }
    }

    pub async fn get_device(&self, device_id: &str) -> GuardResult<Option<GbDeviceRecord>> {
        match self {
            Self::Mysql(store) => store.get_gb_device(device_id).await,
            Self::Sqlite(store) => store.get_gb_device(device_id).await,
        }
    }

    pub async fn upsert_device(&self, device: &GbDeviceRecord) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.upsert_gb_device(device).await,
            Self::Sqlite(store) => store.upsert_gb_device(device).await,
        }
    }

    pub async fn delete_device(&self, device_id: &str) -> GuardResult<bool> {
        match self {
            Self::Mysql(store) => store.delete_gb_device(device_id).await,
            Self::Sqlite(store) => store.delete_gb_device(device_id).await,
        }
    }

    pub async fn list_channels(&self, device_id: &str) -> GuardResult<Vec<GbChannelRecord>> {
        match self {
            Self::Mysql(store) => store.list_gb_channels(device_id).await,
            Self::Sqlite(store) => store.list_gb_channels(device_id).await,
        }
    }

    pub async fn get_channel(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Option<GbChannelRecord>> {
        match self {
            Self::Mysql(store) => store.get_gb_channel(device_id, channel_id).await,
            Self::Sqlite(store) => store.get_gb_channel(device_id, channel_id).await,
        }
    }

    pub async fn upsert_channel(&self, channel: &GbChannelRecord) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.upsert_gb_channel(channel).await,
            Self::Sqlite(store) => store.upsert_gb_channel(channel).await,
        }
    }

    pub async fn delete_channel(&self, device_id: &str, channel_id: &str) -> GuardResult<bool> {
        match self {
            Self::Mysql(store) => store.delete_gb_channel(device_id, channel_id).await,
            Self::Sqlite(store) => store.delete_gb_channel(device_id, channel_id).await,
        }
    }

    pub async fn list_channel_images(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Vec<GbChannelImageRecord>> {
        match self {
            Self::Mysql(store) => store.list_gb_channel_images(device_id, channel_id).await,
            Self::Sqlite(store) => store.list_gb_channel_images(device_id, channel_id).await,
        }
    }

    pub async fn insert_channel_image(&self, image: &GbChannelImageRecord) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.insert_gb_channel_image(image).await,
            Self::Sqlite(store) => store.insert_gb_channel_image(image).await,
        }
    }
}

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

    pub async fn revoke_ui_sessions(&self, username: &str) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.revoke_ui_sessions(username).await,
            Self::Sqlite(store) => store.revoke_ui_sessions(username).await,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MediaRepository {
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl MediaRepository {
    pub async fn running_record_exists(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<bool> {
        match self {
            Self::Mysql(store) => store.running_record_exists(device_id, channel_id).await,
            Self::Sqlite(store) => store.running_record_exists(device_id, channel_id).await,
        }
    }

    pub async fn insert_record(&self, record: &GmvRecordInsert) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.insert_record(record).await,
            Self::Sqlite(store) => store.insert_record(record).await,
        }
    }

    pub async fn finish_record(&self, file: &RecordFileInsert) -> GuardResult<bool> {
        match self {
            Self::Mysql(store) => store.finish_record(file).await,
            Self::Sqlite(store) => store.finish_record(file).await,
        }
    }

    pub async fn insert_media_file(&self, file: &MediaFileInsert) -> GuardResult<()> {
        match self {
            Self::Mysql(store) => store.insert_media_file(file).await,
            Self::Sqlite(store) => store.insert_media_file(file).await,
        }
    }
}

pub enum PersistentStore {
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl PersistentStore {
    pub async fn connect(config: &GuardAppConfig) -> GuardResult<Self> {
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
        if self.load_users().await?.is_empty() {
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

    pub fn media_repository(&self) -> MediaRepository {
        match self {
            Self::Mysql(store) => MediaRepository::Mysql(store.clone()),
            Self::Sqlite(store) => MediaRepository::Sqlite(store.clone()),
        }
    }

    pub fn gb_repository(&self) -> GbRepository {
        match self {
            Self::Mysql(store) => GbRepository::Mysql(store.clone()),
            Self::Sqlite(store) => GbRepository::Sqlite(store.clone()),
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
