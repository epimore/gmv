use base::serde::Deserialize;
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct CenterConfig {
    pub endpoint: String,
    #[serde(default)]
    pub allow_plaintext: bool,
    #[serde(default)]
    pub tls: Option<CenterTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
pub struct CenterTlsConfig {
    pub ca_certificate_path: PathBuf,
    pub client_certificate_path: PathBuf,
    pub client_private_key_path: PathBuf,
    #[serde(default)]
    pub domain_name: Option<String>,
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
            let url = url::Url::parse(&center.endpoint)
                .map_err(|_| "gmvc.endpoint must be a valid URL".to_string())?;
            if url.scheme() != "https" && !(center.allow_plaintext && url.scheme() == "http") {
                return Err(
                    "gmvc.endpoint requires TLS unless allow_plaintext is explicit".to_string(),
                );
            }
            if url.scheme() == "https" && center.tls.is_none() {
                return Err("gmvc.tls is required for mutual TLS".to_string());
            }
            if url.scheme() == "http" && center.tls.is_some() {
                return Err("gmvc.tls cannot be used with a plaintext endpoint".to_string());
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
