use crate::core::{GuardError, GuardResult};

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
            if !matches!(
                action.as_str(),
                "stream.start"
                    | "stream.stop"
                    | "stream.playback"
                    | "stream.download"
                    | "device.talk"
                    | "device.ptz"
                    | "ai.start"
                    | "ai.cancel"
                    | "playback.ticket.renew"
            ) || !actions.insert(action)
            {
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
