mod artifact;
mod center;
mod config;
mod guard;
mod inventory;
mod state;
mod status;

pub use config::StewardConfig;

use crate::{
    artifact::ArtifactService,
    center::{CenterConnector, CenterConnectorConfig},
    inventory::{InventoryService, SystemdInventoryAdapter},
    state::StateStore,
    status::SharedStatus,
};
use base::utils::rt::GlobalRuntime;
use gmv_protocol::guard::v1::CenterConnectionState;
use std::sync::Arc;

pub async fn run(
    config: StewardConfig,
    runtime: GlobalRuntime,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    config.validate()?;
    let manifest = config.load_manifest()?;
    std::fs::create_dir_all(&config.staging_root)?;
    let store = StateStore::open(&config.database_path).await?;
    let inventory = InventoryService::new(manifest, Arc::new(SystemdInventoryAdapter));
    let first_inventory = inventory.refresh().await;
    store.save_inventory(&first_inventory).await?;
    let instance_id = gmv_nodec::generate_instance_id();
    let status = SharedStatus::new(config.installation_id.clone(), instance_id.clone());
    let artifacts = ArtifactService::from_config(&config)?;

    if let Some(endpoint) = config.guard_endpoint.as_deref() {
        guard::start_guard_reporter(
            guard::GuardReporterConfig {
                endpoint: endpoint.to_string(),
                installation_id: config.installation_id.clone(),
                node_id: config.node_id.clone(),
                instance_id: instance_id.clone(),
            },
            inventory.clone(),
            store.clone(),
            status.clone(),
            &runtime,
        )?;
    }

    if let Some(center_config) = config.gmvc.clone() {
        let connector = CenterConnector::new(
            CenterConnectorConfig {
                center: center_config,
                installation_id: config.installation_id,
                node_id: config.node_id,
                instance_id,
            },
            inventory,
            store.clone(),
            status.clone(),
            artifacts,
        )?;
        let center_cancel = runtime.cancel.clone();
        runtime.spawn("steward-gmvc-supervisor", async move {
            connector.run(center_cancel).await;
        })?;
    } else {
        status
            .update(|snapshot| snapshot.center_connection = CenterConnectionState::Disabled as i32);
    }

    runtime.cancel.cancelled().await;
    store.close().await;
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
