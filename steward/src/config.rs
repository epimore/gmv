use base::serde::Deserialize;
use std::{collections::HashSet, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct StewardConfig {
    pub installation_id: String,
    pub node_id: String,
    pub manifest_path: PathBuf,
    pub database_path: PathBuf,
    pub staging_root: PathBuf,
    #[serde(default = "default_max_artifact_bytes")]
    pub max_artifact_bytes: u64,
    #[serde(default = "default_artifact_timeout_secs")]
    pub artifact_timeout_secs: u64,
    #[serde(default)]
    pub artifact_allowed_hosts: Vec<String>,
    #[serde(default)]
    pub trusted_signing_keys: Vec<TrustedSigningKey>,
    #[serde(default)]
    pub guard_endpoint: Option<String>,
    #[serde(default)]
    pub gmvc: Option<CenterConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct CenterConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_mqtt_keep_alive_secs")]
    pub keep_alive_secs: u64,
    #[serde(default = "default_mqtt_request_capacity")]
    pub request_capacity: usize,
    #[serde(default = "default_mqtt_session_expiry_secs")]
    pub session_expiry_secs: u64,
    #[serde(default = "default_mqtt_topic_prefix")]
    pub topic_prefix: String,
    #[serde(default)]
    pub allow_plaintext: bool,
    #[serde(default)]
    pub tls: Option<CenterTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct CenterTlsConfig {
    pub ca_certificate_path: PathBuf,
    #[serde(default)]
    pub client_certificate_path: Option<PathBuf>,
    #[serde(default)]
    pub client_private_key_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct TrustedSigningKey {
    pub key_id: String,
    pub public_key_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct InstallManifest {
    pub revision: String,
    pub components: Vec<ComponentSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct ComponentSpec {
    pub component_id: String,
    pub service_type: String,
    pub systemd_unit: String,
    pub version: String,
}

impl StewardConfig {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = std::env::var_os("GMV_STEWARD_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("steward.yml"));
        let bytes = std::fs::read(&path)?;
        let config: Self = base::serde_yaml::from_slice(&bytes)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_identifier("installation_id", &self.installation_id)?;
        validate_identifier("node_id", &self.node_id)?;
        if self.manifest_path.as_os_str().is_empty()
            || self.database_path.as_os_str().is_empty()
            || self.staging_root.as_os_str().is_empty()
        {
            return Err("manifest_path, database_path and staging_root are required".to_string());
        }
        if let Some(center) = &self.gmvc {
            if center.host.trim().is_empty() || center.port == 0 {
                return Err("gmvc.host and gmvc.port are required".to_string());
            }
            if center.keep_alive_secs == 0
                || center.request_capacity == 0
                || center.session_expiry_secs == 0
            {
                return Err("gmvc MQTT timing and capacity values must be positive".to_string());
            }
            if center.username.is_some() != center.password.is_some() {
                return Err(
                    "gmvc.username and gmvc.password must be configured together".to_string(),
                );
            }
            gmv_protocol::steward_mqtt::center_upstream_filter(&center.topic_prefix)
                .map_err(str::to_string)?;
            if center.tls.is_none() && !(center.allow_plaintext && is_loopback_host(&center.host)) {
                return Err(
                    "gmvc MQTT requires TLS unless loopback plaintext is explicit".to_string(),
                );
            }
            if let Some(tls) = &center.tls {
                if tls.ca_certificate_path.as_os_str().is_empty() {
                    return Err("gmvc.tls.ca_certificate_path is required".to_string());
                }
                if tls.client_certificate_path.is_some() != tls.client_private_key_path.is_some() {
                    return Err(
                        "gmvc MQTT client certificate and private key must be configured together"
                            .to_string(),
                    );
                }
            }
            if self.max_artifact_bytes == 0
                || self.artifact_timeout_secs == 0
                || self.artifact_allowed_hosts.is_empty()
                || self.trusted_signing_keys.is_empty()
            {
                return Err("GMVC delivery requires artifact bounds, allowed hosts and trusted signing keys".to_string());
            }
            let mut key_ids = HashSet::new();
            for key in &self.trusted_signing_keys {
                validate_identifier("signing key id", &key.key_id)?;
                if !key_ids.insert(key.key_id.as_str()) {
                    return Err(format!("duplicate signing key id: {}", key.key_id));
                }
            }
        }
        Ok(())
    }

    pub fn load_manifest(
        &self,
    ) -> Result<InstallManifest, Box<dyn std::error::Error + Send + Sync>> {
        let bytes = std::fs::read(&self.manifest_path)?;
        let manifest: InstallManifest = base::serde_yaml::from_slice(&bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }
}

fn default_max_artifact_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}

fn default_artifact_timeout_secs() -> u64 {
    600
}

fn default_mqtt_keep_alive_secs() -> u64 {
    30
}

fn default_mqtt_request_capacity() -> usize {
    64
}

fn default_mqtt_session_expiry_secs() -> u64 {
    24 * 60 * 60
}

fn default_mqtt_topic_prefix() -> String {
    gmv_protocol::steward_mqtt::DEFAULT_TOPIC_PREFIX.to_string()
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl InstallManifest {
    pub fn validate(&self) -> Result<(), String> {
        validate_identifier("manifest revision", &self.revision)?;
        if self.components.is_empty() {
            return Err("installation manifest must declare at least one component".to_string());
        }
        let mut component_ids = HashSet::new();
        let mut units = HashSet::new();
        for component in &self.components {
            validate_identifier("component_id", &component.component_id)?;
            validate_identifier("service_type", &component.service_type)?;
            validate_systemd_unit(&component.systemd_unit)?;
            if component.version.trim().is_empty() {
                return Err("component version is required".to_string());
            }
            if !component_ids.insert(component.component_id.as_str()) {
                return Err(format!(
                    "duplicate component_id: {}",
                    component.component_id
                ));
            }
            if !units.insert(component.systemd_unit.as_str()) {
                return Err(format!(
                    "duplicate systemd_unit: {}",
                    component.systemd_unit
                ));
            }
        }
        Ok(())
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    valid
        .then_some(())
        .ok_or_else(|| format!("{label} contains unsupported characters"))
}

fn validate_systemd_unit(value: &str) -> Result<(), String> {
    validate_identifier("systemd_unit", value)?;
    if !value.ends_with(".service") {
        return Err("systemd_unit must end with .service".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_shell_like_unit_names() {
        let manifest = InstallManifest {
            revision: "r1".to_string(),
            components: vec![ComponentSpec {
                component_id: "avai".to_string(),
                service_type: "avai".to_string(),
                systemd_unit: "gmv-avai.service;reboot".to_string(),
                version: "1".to_string(),
            }],
        };
        assert!(manifest.validate().is_err());
    }
}
