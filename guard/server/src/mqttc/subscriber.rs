use std::collections::HashSet;
use std::sync::Arc;

use base::serde::Deserialize;
use base::serde_json::Value;
use parking_lot::Mutex;

use crate::core::{GuardError, GuardResult};
use crate::mqttc::mapping::{CommandAction, RoutedCommand};
use crate::store::{InMemoryGuardStore, mysql::MysqlStore, sqlite::SqliteStore};

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct MqttCommand {
    pub command_id: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub enum CommandIdRepository {
    Memory(InMemoryGuardStore),
    Mysql(MysqlStore),
    Sqlite(SqliteStore),
}

impl From<InMemoryGuardStore> for CommandIdRepository {
    fn from(store: InMemoryGuardStore) -> Self {
        Self::Memory(store)
    }
}
impl From<MysqlStore> for CommandIdRepository {
    fn from(store: MysqlStore) -> Self {
        Self::Mysql(store)
    }
}
impl From<SqliteStore> for CommandIdRepository {
    fn from(store: SqliteStore) -> Self {
        Self::Sqlite(store)
    }
}

impl CommandIdRepository {
    async fn claim(&self, command_id: &str, expires_at_ms: i64, now_ms: i64) -> GuardResult<bool> {
        match self {
            Self::Memory(store) => Ok(store.claim_command(command_id, expires_at_ms, now_ms)),
            Self::Mysql(store) => store.claim_command(command_id, expires_at_ms, now_ms).await,
            Self::Sqlite(store) => store.claim_command(command_id, expires_at_ms, now_ms).await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MqttCommandPolicy {
    allowed_actions: HashSet<String>,
    seen: Arc<Mutex<HashSet<String>>>,
    max_ttl_ms: i64,
}

impl MqttCommandPolicy {
    pub fn new(
        allowed_actions: impl IntoIterator<Item = String>,
        max_ttl_ms: i64,
    ) -> GuardResult<Self> {
        if max_ttl_ms <= 0 {
            return Err(GuardError::InvalidConfig(
                "MQTT command max TTL must be positive".to_string(),
            ));
        }
        Ok(Self {
            allowed_actions: allowed_actions.into_iter().collect(),
            seen: Arc::new(Mutex::new(HashSet::new())),
            max_ttl_ms,
        })
    }

    pub fn decode(&self, payload: &[u8], now_ms: i64) -> GuardResult<Option<RoutedCommand>> {
        let (command, routed) = self.validate(payload, now_ms)?;
        let mut seen = self.seen.lock();
        if !seen.insert(command.command_id) {
            return Ok(None);
        }
        Ok(Some(routed))
    }

    pub async fn decode_with_repository(
        &self,
        payload: &[u8],
        now_ms: i64,
        repository: &CommandIdRepository,
    ) -> GuardResult<Option<RoutedCommand>> {
        let (command, routed) = self.validate(payload, now_ms)?;
        if !repository
            .claim(&command.command_id, command.expires_at_ms, now_ms)
            .await?
        {
            return Ok(None);
        }
        Ok(Some(routed))
    }

    fn validate(&self, payload: &[u8], now_ms: i64) -> GuardResult<(MqttCommand, RoutedCommand)> {
        let command: MqttCommand = base::serde_json::from_slice(payload).map_err(|error| {
            GuardError::InvalidConfig(format!("invalid MQTT command JSON: {error}"))
        })?;
        if command.command_id.is_empty()
            || command.command_id.len() > 128
            || command.command_id.chars().any(char::is_whitespace)
            || command.target.is_empty()
        {
            return Err(GuardError::InvalidConfig(
                "MQTT command_id and target are invalid".to_string(),
            ));
        }
        if command.expires_at_ms < command.issued_at_ms
            || command.expires_at_ms.saturating_sub(command.issued_at_ms) > self.max_ttl_ms
            || now_ms > command.expires_at_ms
        {
            return Err(GuardError::InvalidConfig(
                "MQTT command TTL is invalid or expired".to_string(),
            ));
        }
        if !self.allowed_actions.contains(&command.action) {
            return Err(GuardError::InvalidIdentity(
                "MQTT command action is not allowed".to_string(),
            ));
        }
        let action = CommandAction::parse(&command.action).ok_or_else(|| {
            GuardError::InvalidConfig("MQTT command action is unsupported".to_string())
        })?;
        let routed = RoutedCommand {
            command_id: command.command_id.clone(),
            action,
            target: command.target.clone(),
            payload: command.payload.clone(),
        };
        Ok((command, routed))
    }
}
