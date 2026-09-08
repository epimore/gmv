use crate::config::InstallManifest;
use gmv_protocol::steward::v1::{ComponentObservation, ComponentState, InventorySnapshot};
use std::sync::Arc;
use tonic::async_trait;

#[async_trait]
pub trait InventoryAdapter: Send + Sync {
    async fn observe(
        &self,
        unit: &str,
    ) -> Result<ServiceObservation, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceObservation {
    pub loaded: bool,
    pub active: bool,
    pub version: Option<String>,
}

#[derive(Clone)]
pub struct InventoryService {
    manifest: Arc<InstallManifest>,
    adapter: Arc<dyn InventoryAdapter>,
}

impl InventoryService {
    pub fn new(manifest: InstallManifest, adapter: Arc<dyn InventoryAdapter>) -> Self {
        Self {
            manifest: Arc::new(manifest),
            adapter,
        }
    }

    pub fn manifest_revision(&self) -> &str {
        &self.manifest.revision
    }

    pub async fn refresh(&self) -> InventorySnapshot {
        let mut components = Vec::with_capacity(self.manifest.components.len());
        for component in &self.manifest.components {
            let observation = self.adapter.observe(&component.systemd_unit).await;
            let (state, observed_version, reason) = match observation {
                Ok(observation) if !observation.loaded => (
                    ComponentState::Missing,
                    observation.version.unwrap_or_default(),
                    "unit_not_loaded".to_string(),
                ),
                Ok(observation) if !observation.active => (
                    ComponentState::Stopped,
                    observation.version.unwrap_or_default(),
                    "unit_not_active".to_string(),
                ),
                Ok(observation)
                    if observation.version.as_deref().is_some_and(|version| {
                        !version.is_empty() && version != component.version
                    }) =>
                {
                    (
                        ComponentState::VersionMismatch,
                        observation.version.unwrap_or_default(),
                        "version_mismatch".to_string(),
                    )
                }
                Ok(observation) => (
                    ComponentState::Running,
                    observation.version.unwrap_or_default(),
                    String::new(),
                ),
                Err(_) => (
                    ComponentState::Unknown,
                    String::new(),
                    "observation_failed".to_string(),
                ),
            };
            components.push(ComponentObservation {
                component_id: component.component_id.clone(),
                service_type: component.service_type.clone(),
                declared_version: component.version.clone(),
                observed_version,
                state: state as i32,
                stable_reason: reason,
            });
        }
        InventorySnapshot {
            manifest_revision: self.manifest.revision.clone(),
            observed_at_epoch_ms: crate::now_ms(),
            components,
        }
    }
}

#[derive(Debug, Default)]
pub struct SystemdInventoryAdapter;

#[async_trait]
impl InventoryAdapter for SystemdInventoryAdapter {
    async fn observe(
        &self,
        unit: &str,
    ) -> Result<ServiceObservation, Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(target_os = "linux")]
        {
            let output = base::tokio::process::Command::new("systemctl")
                .args([
                    "show",
                    unit,
                    "--property=LoadState",
                    "--property=ActiveState",
                    "--property=Environment",
                    "--value",
                ])
                .kill_on_drop(true)
                .output()
                .await?;
            let stdout = String::from_utf8(output.stdout)?;
            let mut lines = stdout.lines();
            let load_state = lines.next().unwrap_or_default();
            let active_state = lines.next().unwrap_or_default();
            let environment = lines.next().unwrap_or_default();
            let version = environment
                .split_ascii_whitespace()
                .find_map(|item| item.strip_prefix("GMV_VERSION="))
                .map(str::to_string);
            Ok(ServiceObservation {
                loaded: output.status.success() && load_state == "loaded",
                active: active_state == "active",
                version,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = unit;
            Err("systemd inventory is only supported on Linux".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ComponentSpec;

    struct FakeAdapter;

    #[async_trait]
    impl InventoryAdapter for FakeAdapter {
        async fn observe(
            &self,
            unit: &str,
        ) -> Result<ServiceObservation, Box<dyn std::error::Error + Send + Sync>> {
            Ok(match unit {
                "gmv-avai.service" => ServiceObservation {
                    loaded: true,
                    active: true,
                    version: Some("2".to_string()),
                },
                _ => ServiceObservation {
                    loaded: false,
                    active: false,
                    version: None,
                },
            })
        }
    }

    #[tokio::test]
    async fn reports_declared_components_without_discovering_unmanaged_processes() {
        let service = InventoryService::new(
            InstallManifest {
                revision: "m1".to_string(),
                components: vec![ComponentSpec {
                    component_id: "avai".to_string(),
                    service_type: "avai".to_string(),
                    systemd_unit: "gmv-avai.service".to_string(),
                    version: "1".to_string(),
                }],
            },
            Arc::new(FakeAdapter),
        );
        let snapshot = service.refresh().await;
        assert_eq!(snapshot.components.len(), 1);
        assert_eq!(
            snapshot.components[0].state,
            ComponentState::VersionMismatch as i32
        );
    }
}
