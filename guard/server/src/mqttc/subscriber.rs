use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base::serde::Deserialize;
use base::serde_json::Value;
use parking_lot::Mutex;

use crate::core::{GuardError, GuardResult};
use crate::mqttc::mapping::{CommandAction, RoutedCommand};
use crate::store::InMemoryGuardStore;
#[cfg(feature = "db-mysql")]
use crate::store::mysql::MysqlStore;
#[cfg(feature = "db-sqlite")]
use crate::store::sqlite::SqliteStore;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct MqttCommand {
    #[serde(default)]
    pub integration_id: String,
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
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

impl From<InMemoryGuardStore> for CommandIdRepository {
    fn from(store: InMemoryGuardStore) -> Self {
        Self::Memory(store)
    }
}
#[cfg(feature = "db-mysql")]
impl From<MysqlStore> for CommandIdRepository {
    fn from(store: MysqlStore) -> Self {
        Self::Mysql(store)
    }
}
#[cfg(feature = "db-sqlite")]
impl From<SqliteStore> for CommandIdRepository {
    fn from(store: SqliteStore) -> Self {
        Self::Sqlite(store)
    }
}

impl CommandIdRepository {
    async fn claim(&self, command_id: &str, expires_at_ms: i64, now_ms: i64) -> GuardResult<bool> {
        match self {
            Self::Memory(store) => Ok(store.claim_command(command_id, expires_at_ms, now_ms)),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.claim_command(command_id, expires_at_ms, now_ms).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.claim_command(command_id, expires_at_ms, now_ms).await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MqttCommandPolicy {
    allowed_actions: HashSet<String>,
    topic_routes: HashMap<String, (String, HashSet<String>)>,
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
            topic_routes: HashMap::new(),
            seen: Arc::new(Mutex::new(HashSet::new())),
            max_ttl_ms,
        })
    }

    pub fn with_topic_routes(
        mut self,
        routes: impl IntoIterator<Item = (String, String, Vec<String>)>,
    ) -> GuardResult<Self> {
        for (topic, integration_id, actions) in routes {
            if topic.is_empty()
                || topic.contains(['#', '+'])
                || integration_id.is_empty()
                || self
                    .topic_routes
                    .insert(topic, (integration_id, actions.into_iter().collect()))
                    .is_some()
            {
                return Err(GuardError::InvalidConfig(
                    "invalid or duplicate MQTT integration command route".to_string(),
                ));
            }
        }
        Ok(self)
    }

    pub fn decode(&self, payload: &[u8], now_ms: i64) -> GuardResult<Option<RoutedCommand>> {
        let (command, routed) = self.validate(None, payload, now_ms)?;
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
        let (command, routed) = self.validate(None, payload, now_ms)?;
        if !repository
            .claim(&command.command_id, command.expires_at_ms, now_ms)
            .await?
        {
            return Ok(None);
        }
        Ok(Some(routed))
    }

    pub async fn decode_topic_with_repository(
        &self,
        topic: &str,
        payload: &[u8],
        now_ms: i64,
        repository: &CommandIdRepository,
    ) -> GuardResult<Option<RoutedCommand>> {
        let (command, routed) = self.validate(Some(topic), payload, now_ms)?;
        if !repository
            .claim(&command.command_id, command.expires_at_ms, now_ms)
            .await?
        {
            return Ok(None);
        }
        Ok(Some(routed))
    }

    fn validate(
        &self,
        topic: Option<&str>,
        payload: &[u8],
        now_ms: i64,
    ) -> GuardResult<(MqttCommand, RoutedCommand)> {
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
        let route_actions = topic
            .and_then(|topic| self.topic_routes.get(topic))
            .map(|(integration_id, actions)| {
                if command.integration_id != *integration_id {
                    return Err(GuardError::InvalidIdentity(
                        "MQTT command integration does not match topic".to_string(),
                    ));
                }
                Ok(actions)
            })
            .transpose()?;
        let action_allowed = route_actions.map_or_else(
            || self.allowed_actions.contains(&command.action),
            |actions| actions.contains(&command.action),
        );
        if !action_allowed {
            return Err(GuardError::InvalidIdentity(
                "MQTT command action is not allowed".to_string(),
            ));
        }
        let action = CommandAction::parse(&command.action).ok_or_else(|| {
            GuardError::InvalidConfig("MQTT command action is unsupported".to_string())
        })?;
        let routed = RoutedCommand {
            command_id: command.command_id.clone(),
            integration_id: command.integration_id.clone(),
            expires_at_ms: command.expires_at_ms,
            action,
            target: command.target.clone(),
            payload: command.payload.clone(),
        };
        Ok((command, routed))
    }
}
