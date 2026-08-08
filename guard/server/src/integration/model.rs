use crate::core::{GuardError, GuardResult};

pub const MQTT_COMMAND_ACTIONS: &[&str] = &[
    "stream.start",
    "stream.stop",
    "stream.playback",
    "stream.download",
    "device.broadcast",
    "device.ptz",
    "ai.start",
    "ai.cancel",
    "playback.ticket.renew",
    "system.dashboard.get",
    "media.transport.get",
    "media.operation.list",
    "media.operation.get",
    "media.operation.continue",
    "media.operation.cancel",
    "node.list",
    "lease.list",
    "gb.session_config.get",
    "gb.device.list",
    "gb.device.create",
    "gb.device.get",
    "gb.device.update",
    "gb.device.delete",
    "gb.channel.list",
    "gb.channel.get",
    "gb.channel.update",
    "gb.resource.list",
    "gb.resource.confirm",
    "gb.resource.reset",
    "gb.image.list",
    "gb.image.snapshot",
    "gb.image.access",
    "gb.image.cover",
    "gb.record.list",
    "gb.record.query",
    "cloud_recording.list",
    "cloud_recording.create",
    "cloud_recording.get",
    "cloud_recording.stop",
    "cloud_recording.delete",
    "cloud_recording.access",
    "broadcast.start",
    "broadcast.get",
    "broadcast.stop_target",
    "broadcast.stop_all",
    "device.list",
    "stream.list",
    "gb.stream.list",
    "gb.stream.management",
    "gb.stream.history",
    "gb.stream.stop",
    "stream.release",
    "stream.speed.set",
    "playback.seek",
    "playback.speed.set",
    "playback.state.set",
    "playback.presence.heartbeat",
    "stream.output.list",
    "stream.output.create",
    "stream.output.close",
    "ai.list",
    "runtime.status.get",
];

pub fn mqtt_action_scope(action: &str) -> Option<&'static str> {
    match action {
        "system.dashboard.get" | "runtime.status.get" => Some("runtime:read"),
        "media.transport.get" | "media.operation.list" | "media.operation.get" => {
            Some("streams:read")
        }
        "media.operation.continue" | "media.operation.cancel" => Some("streams:write"),
        "node.list" => Some("nodes:read"),
        "lease.list" => Some("leases:read"),
        "gb.session_config.get"
        | "gb.device.list"
        | "gb.device.get"
        | "gb.channel.list"
        | "gb.channel.get"
        | "gb.resource.list"
        | "device.list" => Some("devices:read"),
        "gb.device.create"
        | "gb.device.update"
        | "gb.device.delete"
        | "gb.channel.update"
        | "gb.resource.confirm"
        | "gb.resource.reset" => Some("devices:write"),
        "device.ptz" => Some("devices:control"),
        "gb.image.list" | "gb.image.access" => Some("images:read"),
        "gb.image.snapshot" | "gb.image.cover" => Some("devices:control"),
        "gb.record.list" | "cloud_recording.list" | "cloud_recording.get" => {
            Some("recordings:read")
        }
        "gb.record.query"
        | "cloud_recording.create"
        | "cloud_recording.stop"
        | "cloud_recording.delete"
        | "cloud_recording.access" => Some("recordings:write"),
        "device.broadcast"
        | "broadcast.start"
        | "broadcast.get"
        | "broadcast.stop_target"
        | "broadcast.stop_all" => Some("audio:control"),
        "stream.start" => Some("streams:preview"),
        "stream.playback"
        | "playback.seek"
        | "playback.speed.set"
        | "playback.state.set"
        | "playback.presence.heartbeat"
        | "playback.ticket.renew" => Some("streams:playback"),
        "stream.download"
        | "stream.stop"
        | "gb.stream.stop"
        | "stream.release"
        | "stream.speed.set"
        | "stream.output.create"
        | "stream.output.close" => Some("streams:write"),
        "stream.list" | "gb.stream.list" | "gb.stream.management" | "stream.output.list" => {
            Some("streams:read")
        }
        "gb.stream.history" => Some("devices:read"),
        "ai.list" => Some("ai:read"),
        "ai.start" | "ai.cancel" => Some("ai:write"),
        _ => None,
    }
}

pub fn mqtt_action_for_http(method: &str, path: &str) -> Option<&'static str> {
    match (method, path) {
        ("get", "/dashboard") => Some("system.dashboard.get"),
        ("get", "/media/transport") => Some("media.transport.get"),
        ("get", "/media/operations") => Some("media.operation.list"),
        ("get", "/media/operations/{operation_id}") => Some("media.operation.get"),
        ("post", "/media/operations/{operation_id}/continue") => Some("media.operation.continue"),
        ("post", "/media/operations/{operation_id}/cancel") => Some("media.operation.cancel"),
        ("get", "/nodes") => Some("node.list"),
        ("get", "/leases") => Some("lease.list"),
        ("get", "/gb28181/session-nodes/{node_id}/config") => Some("gb.session_config.get"),
        ("get", "/gb28181/devices") => Some("gb.device.list"),
        ("post", "/gb28181/devices") => Some("gb.device.create"),
        ("get", "/gb28181/devices/{device_id}") => Some("gb.device.get"),
        ("post", "/gb28181/devices/{device_id}") => Some("gb.device.update"),
        ("post", "/gb28181/devices/{device_id}/delete") => Some("gb.device.delete"),
        ("get", "/gb28181/devices/{device_id}/channels") => Some("gb.channel.list"),
        ("get", "/gb28181/devices/{device_id}/resources") => Some("gb.resource.list"),
        ("post", "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation") => {
            Some("gb.resource.confirm")
        }
        ("post", "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset") => {
            Some("gb.resource.reset")
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}") => Some("gb.channel.get"),
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}") => Some("gb.channel.update"),
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/preview") => {
            Some("stream.start")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/playback") => {
            Some("stream.playback")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/ptz") => Some("device.ptz"),
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/images") => {
            Some("gb.image.list")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images") => {
            Some("gb.image.snapshot")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access") => {
            Some("gb.image.access")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover") => {
            Some("gb.image.cover")
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/records") => {
            Some("gb.record.list")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/records/query") => {
            Some("gb.record.query")
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings") => {
            Some("cloud_recording.list")
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings") => {
            Some("cloud_recording.create")
        }
        ("get", "/gb28181/cloud-recordings/{task_id}") => Some("cloud_recording.get"),
        ("post", "/gb28181/cloud-recordings/{task_id}/stop") => Some("cloud_recording.stop"),
        ("post", "/gb28181/cloud-recordings/{task_id}/delete") => Some("cloud_recording.delete"),
        ("post", "/gb28181/cloud-recordings/{task_id}/access") => Some("cloud_recording.access"),
        ("post", "/gb28181/broadcasts/start") => Some("broadcast.start"),
        ("get", "/gb28181/broadcasts/{broadcast_id}") => Some("broadcast.get"),
        ("post", "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop") => {
            Some("broadcast.stop_target")
        }
        ("post", "/gb28181/broadcasts/{broadcast_id}/stop-all") => Some("broadcast.stop_all"),
        ("get", "/devices") => Some("device.list"),
        ("post", "/devices/{device_id}/preview") => Some("stream.start"),
        ("post", "/devices/{device_id}/playback") => Some("stream.playback"),
        ("post", "/devices/{device_id}/download") => Some("stream.download"),
        ("post", "/devices/{device_id}/ptz") => Some("device.ptz"),
        ("get", "/streams") => Some("stream.list"),
        ("get", "/gb28181/streams") => Some("gb.stream.list"),
        ("get", "/gb28181/streams/{stream_id}/management") => Some("gb.stream.management"),
        ("get", "/gb28181/stream-history") => Some("gb.stream.history"),
        ("post", "/gb28181/streams/{stream_id}/stop") => Some("gb.stream.stop"),
        ("post", "/streams/{stream_id}/stop") => Some("stream.stop"),
        ("post", "/streams/{stream_id}/release") => Some("stream.release"),
        ("post", "/streams/{stream_id}/speed") => Some("stream.speed.set"),
        ("post", "/playbacks/{playback_id}/seek") => Some("playback.seek"),
        ("post", "/playbacks/{playback_id}/speed") => Some("playback.speed.set"),
        ("post", "/playbacks/{playback_id}/state") => Some("playback.state.set"),
        ("post", "/playbacks/presence/heartbeat") => Some("playback.presence.heartbeat"),
        ("post", "/playback-tickets/{token}/renew") => Some("playback.ticket.renew"),
        ("get", "/streams/{stream_id}/outputs") => Some("stream.output.list"),
        ("post", "/streams/{stream_id}/outputs") => Some("stream.output.create"),
        ("post", "/streams/{stream_id}/outputs/{output_id}/close") => Some("stream.output.close"),
        ("get", "/ai/tasks") => Some("ai.list"),
        ("post", "/ai/tasks") => Some("ai.start"),
        ("post", "/ai/tasks/{task_id}/cancel") => Some("ai.cancel"),
        ("get", "/runtime/status") => Some("runtime.status.get"),
        _ => None,
    }
}

pub fn mqtt_special_for_http(method: &str, path: &str) -> Option<&'static str> {
    match (method, path) {
        ("get", "/events") => {
            Some("MQTT 使用 gmv/events/{integration_id}/{event_type} 推送替代 HTTP 历史轮询")
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum IntegrationTransport {
    Http,
    Mqtt,
}

impl IntegrationTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Mqtt => "MQTT",
        }
    }

    pub fn parse(value: &str) -> GuardResult<Self> {
        match value {
            "HTTP" => Ok(Self::Http),
            "MQTT" => Ok(Self::Mqtt),
            _ => Err(GuardError::InvalidConfig(format!(
                "invalid integration transport {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum CredentialPurpose {
    HttpInboundVerify,
    HttpCallbackSign,
}

impl CredentialPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HttpInboundVerify => "HTTP_INBOUND_VERIFY",
            Self::HttpCallbackSign => "HTTP_CALLBACK_SIGN",
        }
    }

    pub fn parse(value: &str) -> GuardResult<Self> {
        match value {
            "HTTP_INBOUND_VERIFY" => Ok(Self::HttpInboundVerify),
            "HTTP_CALLBACK_SIGN" => Ok(Self::HttpCallbackSign),
            _ => Err(GuardError::InvalidConfig(format!(
                "invalid integration credential purpose {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum CredentialStatus {
    Active,
    Revoked,
}

impl CredentialStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Revoked => "REVOKED",
        }
    }

    pub fn parse(value: &str) -> GuardResult<Self> {
        match value {
            "ACTIVE" => Ok(Self::Active),
            "REVOKED" => Ok(Self::Revoked),
            _ => Err(GuardError::InvalidConfig(format!(
                "invalid integration credential status {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct Integration {
    pub integration_id: String,
    pub name: String,
    pub transport: IntegrationTransport,
    pub inbound_enabled: bool,
    pub outbound_enabled: bool,
    pub enabled: bool,
    pub scopes: Vec<String>,
    pub expires_at_ms: Option<i64>,
    pub config_version: i64,
    pub created_by: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Integration {
    pub fn validate(&self, now_ms: i64) -> GuardResult<()> {
        if self.integration_id.trim().is_empty()
            || self.integration_id.len() > 128
            || self.name.trim().is_empty()
            || self.name.len() > 255
        {
            return Err(GuardError::InvalidConfig(
                "integration id or name is invalid".to_string(),
            ));
        }
        if !self.inbound_enabled && !self.outbound_enabled {
            return Err(GuardError::InvalidConfig(
                "integration must enable inbound or outbound".to_string(),
            ));
        }
        if self.scopes.iter().any(|scope| {
            scope.is_empty() || scope.len() > 128 || scope.chars().any(char::is_whitespace)
        }) {
            return Err(GuardError::InvalidConfig(
                "integration scope is invalid".to_string(),
            ));
        }
        if self
            .expires_at_ms
            .is_some_and(|expires_at| expires_at <= now_ms)
        {
            return Err(GuardError::InvalidConfig(
                "integration expiry must be in the future".to_string(),
            ));
        }
        Ok(())
    }

    pub fn permits(&self, scope: &str, now_ms: i64) -> bool {
        self.enabled
            && self
                .expires_at_ms
                .is_none_or(|expires_at| expires_at > now_ms)
            && self.scopes.iter().any(|candidate| candidate == scope)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct IntegrationCredential {
    pub credential_id: String,
    pub access_key: String,
    pub integration_id: String,
    pub purpose: CredentialPurpose,
    pub secret_ciphertext: String,
    pub key_version: i64,
    pub status: CredentialStatus,
    pub not_before_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
    pub created_by: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl std::fmt::Debug for IntegrationCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntegrationCredential")
            .field("credential_id", &self.credential_id)
            .field("access_key", &self.access_key)
            .field("integration_id", &self.integration_id)
            .field("purpose", &self.purpose)
            .field("secret_ciphertext", &"<redacted>")
            .field("key_version", &self.key_version)
            .field("status", &self.status)
            .field("not_before_ms", &self.not_before_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("revoked_at_ms", &self.revoked_at_ms)
            .field("created_by", &self.created_by)
            .field("created_at_ms", &self.created_at_ms)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

impl IntegrationCredential {
    pub fn is_active_at(&self, now_ms: i64) -> bool {
        self.status == CredentialStatus::Active
            && self.revoked_at_ms.is_none()
            && self.not_before_ms <= now_ms
            && self
                .expires_at_ms
                .is_none_or(|expires_at| expires_at > now_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationCredentialSummary {
    pub credential_id: String,
    pub access_key: String,
    pub integration_id: String,
    pub purpose: CredentialPurpose,
    pub key_version: i64,
    pub status: CredentialStatus,
    pub not_before_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
    pub created_by: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<IntegrationCredential> for IntegrationCredentialSummary {
    fn from(value: IntegrationCredential) -> Self {
        Self {
            credential_id: value.credential_id,
            access_key: value.access_key,
            integration_id: value.integration_id,
            purpose: value.purpose,
            key_version: value.key_version,
            status: value.status,
            not_before_ms: value.not_before_ms,
            expires_at_ms: value.expires_at_ms,
            revoked_at_ms: value.revoked_at_ms,
            created_by: value.created_by,
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationHttpConfig {
    pub integration_id: String,
    pub callback_url: Option<String>,
    pub callback_timeout_ms: i64,
    pub private_network_policy: String,
    pub private_network_allowlist: Vec<String>,
    pub max_attempts: i64,
    pub event_ttl_ms: i64,
    pub max_response_bytes: i64,
    pub updated_at_ms: i64,
}

impl IntegrationHttpConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if self.callback_timeout_ms <= 0
            || self.max_attempts <= 0
            || self.event_ttl_ms <= 0
            || self.max_response_bytes <= 0
            || !matches!(self.private_network_policy.as_str(), "deny" | "allowlist")
        {
            return Err(GuardError::InvalidConfig(
                "invalid HTTP integration delivery policy".to_string(),
            ));
        }
        if (self.private_network_policy == "deny" && !self.private_network_allowlist.is_empty())
            || (self.private_network_policy == "allowlist"
                && self.private_network_allowlist.is_empty())
            || self.private_network_allowlist.iter().any(|entry| {
                entry.is_empty()
                    || entry.len() > 255
                    || entry.chars().any(char::is_whitespace)
                    || (entry.parse::<ipnet::IpNet>().is_err()
                        && entry.parse::<std::net::IpAddr>().is_err()
                        && !entry.bytes().all(|value| {
                            value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-' | b'_')
                        }))
            })
        {
            return Err(GuardError::InvalidConfig(
                "invalid HTTP private network allowlist".to_string(),
            ));
        }
        if self
            .callback_url
            .as_deref()
            .is_some_and(|value| !value.starts_with("https://") || value.len() > 512)
        {
            return Err(GuardError::InvalidConfig(
                "HTTP callback_url must use https and not exceed 512 bytes".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationMqttConfig {
    pub integration_id: String,
    pub protocol_version: String,
    pub allowed_actions: Vec<String>,
    pub command_topic: String,
    pub result_topic: String,
    pub event_topic_prefix: String,
    pub updated_at_ms: i64,
}

impl IntegrationMqttConfig {
    pub fn validate(&self) -> GuardResult<()> {
        if !matches!(self.protocol_version.as_str(), "v3" | "v5") {
            return Err(GuardError::InvalidConfig(
                "MQTT protocol_version must be v3 or v5".to_string(),
            ));
        }
        if self.command_topic != format!("gmv/commands/{}", self.integration_id)
            || self.result_topic != format!("gmv/command-results/{}", self.integration_id)
            || self.event_topic_prefix != format!("gmv/events/{}", self.integration_id)
        {
            return Err(GuardError::InvalidConfig(
                "MQTT integration topics must use the fixed integration prefix".to_string(),
            ));
        }
        let mut actions = std::collections::HashSet::new();
        for action in &self.allowed_actions {
            if !MQTT_COMMAND_ACTIONS.contains(&action.as_str()) || !actions.insert(action) {
                return Err(GuardError::InvalidConfig(
                    "MQTT integration allowed_actions contains an invalid or duplicate action"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationMapping {
    pub mapping_id: String,
    pub integration_id: String,
    pub direction: String,
    pub source_type: String,
    pub schema_version: String,
    pub destination_kind: String,
    pub destination: String,
    pub payload_profile: String,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct IntegrationAudit {
    pub audit_id: String,
    pub integration_id: Option<String>,
    pub actor: String,
    pub action: String,
    pub target_id: String,
    pub outcome: String,
    pub detail_summary: String,
    pub created_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn integration() -> Integration {
        Integration {
            integration_id: "partner-1".to_string(),
            name: "Partner".to_string(),
            transport: IntegrationTransport::Http,
            inbound_enabled: false,
            outbound_enabled: false,
            enabled: false,
            scopes: vec!["devices:read".to_string()],
            expires_at_ms: None,
            config_version: 1,
            created_by: "admin".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn integration_requires_a_direction_and_valid_scope() {
        let mut value = integration();
        assert!(value.validate(1).is_err());
        value.inbound_enabled = true;
        value.validate(1).unwrap();
        value.scopes = vec!["bad scope".to_string()];
        assert!(value.validate(1).is_err());
    }

    #[test]
    fn credential_debug_redacts_ciphertext() {
        let credential = IntegrationCredential {
            credential_id: "credential-1".to_string(),
            access_key: "access-1".to_string(),
            integration_id: "partner-1".to_string(),
            purpose: CredentialPurpose::HttpInboundVerify,
            secret_ciphertext: "sensitive-ciphertext".to_string(),
            key_version: 1,
            status: CredentialStatus::Active,
            not_before_ms: 0,
            expires_at_ms: None,
            revoked_at_ms: None,
            created_by: "admin".to_string(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(!format!("{credential:?}").contains("sensitive-ciphertext"));
    }

    #[test]
    fn http_callback_url_must_fit_the_mapping_destination_column() {
        let mut value = IntegrationHttpConfig {
            integration_id: "partner-1".to_string(),
            callback_url: Some(format!("https://example.com/{}", "a".repeat(492))),
            callback_timeout_ms: 5_000,
            private_network_policy: "deny".to_string(),
            private_network_allowlist: Vec::new(),
            max_attempts: 5,
            event_ttl_ms: 259_200_000,
            max_response_bytes: 65_536,
            updated_at_ms: 1,
        };
        value.validate().unwrap();
        value.callback_url = Some(format!("https://example.com/{}", "a".repeat(493)));
        assert!(value.validate().is_err());
    }
}
