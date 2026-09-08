use crate::{inventory::InventoryService, state::StateStore, status::SharedStatus};
use base::utils::rt::GlobalRuntime;
use base_rpc::RpcChannelConfig;
use gmv_nodec::{CommandHandler, NodeReporter, NodeReporterConfig};
use gmv_protocol::{
    common::v1::{NodeIdentity, NodeKind},
    guard::v1::{
        CommandResult, CommandStatus, GuardCommand, NodeResourceSnapshot, RegisterNodeRequest,
        StewardStatusQuery,
    },
};
use prost::Message;
use std::sync::Arc;

pub struct GuardReporterConfig {
    pub endpoint: String,
    pub installation_id: String,
    pub node_id: String,
    pub instance_id: String,
}

pub fn start_guard_reporter(
    config: GuardReporterConfig,
    inventory: InventoryService,
    store: StateStore,
    status: SharedStatus,
    runtime: &GlobalRuntime,
) -> Result<(), base::exception::GlobalError> {
    let identity = NodeIdentity {
        node_id: config.node_id,
        instance_id: config.instance_id,
        kind: NodeKind::Steward as i32,
    };
    let register = RegisterNodeRequest {
        identity: Some(identity),
        software_version: env!("CARGO_PKG_VERSION").to_string(),
        started_at_epoch_ms: crate::now_ms(),
        endpoints: Vec::new(),
        capabilities: vec![
            "steward.inventory.refresh.v1".to_string(),
            "steward.health.snapshot.v1".to_string(),
        ],
        startup_snapshot: Some(NodeResourceSnapshot::default()),
        host_metrics: None,
        zone: String::new(),
        takeover: false,
        config: Default::default(),
        installation_id: config.installation_id,
    };
    let handler: CommandHandler = Arc::new(move |command| {
        let inventory = inventory.clone();
        let store = store.clone();
        let status = status.clone();
        Box::pin(async move { handle_command(command, inventory, store, status).await })
    });
    let mut reporter = NodeReporterConfig::new(RpcChannelConfig::new(config.endpoint), register);
    reporter.command_handler = Some(handler);
    NodeReporter::spawn_managed(runtime, reporter, runtime.cancel.clone())
}

async fn handle_command(
    command: GuardCommand,
    inventory: InventoryService,
    store: StateStore,
    status: SharedStatus,
) -> CommandResult {
    match command.command_type.as_str() {
        "steward.inventory.refresh.v1" => {
            let snapshot = inventory.refresh().await;
            let persisted = store.save_inventory(&snapshot).await;
            if persisted.is_err() {
                return failed("inventory_persist_failed");
            }
            succeeded(snapshot.encode_to_vec())
        }
        "steward.health.snapshot.v1" => health_snapshot(&command.payload, &status),
        _ => failed("command_unsupported"),
    }
}

fn health_snapshot(payload: &[u8], status: &SharedStatus) -> CommandResult {
    let Ok(query) = StewardStatusQuery::decode(payload) else {
        return failed("status_query_invalid");
    };
    let snapshot = status.snapshot();
    if query.installation_id.is_empty() || query.installation_id != snapshot.installation_id {
        return failed("installation_mismatch");
    }
    succeeded(snapshot.encode_to_vec())
}

fn succeeded(payload: Vec<u8>) -> CommandResult {
    CommandResult {
        status: CommandStatus::Succeeded as i32,
        payload,
        ..CommandResult::default()
    }
}

fn failed(code: &str) -> CommandResult {
    CommandResult {
        status: CommandStatus::Failed as i32,
        error: Some(gmv_protocol::common::v1::ErrorDetail {
            code: code.to_string(),
            message: code.to_string(),
            metadata: Default::default(),
        }),
        ..CommandResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_snapshot_is_scoped_to_the_requested_installation() {
        let status = SharedStatus::new("installation-1".to_string(), "instance-1".to_string());
        let accepted = health_snapshot(
            &StewardStatusQuery {
                installation_id: "installation-1".to_string(),
            }
            .encode_to_vec(),
            &status,
        );
        assert_eq!(accepted.status, CommandStatus::Succeeded as i32);

        let rejected = health_snapshot(
            &StewardStatusQuery {
                installation_id: "installation-2".to_string(),
            }
            .encode_to_vec(),
            &status,
        );
        assert_eq!(rejected.status, CommandStatus::Failed as i32);
        assert_eq!(
            rejected.error.as_ref().map(|error| error.code.as_str()),
            Some("installation_mismatch")
        );
    }

    #[test]
    fn health_snapshot_rejects_invalid_payload() {
        let status = SharedStatus::new("installation-1".to_string(), "instance-1".to_string());
        let rejected = health_snapshot(&[0xff], &status);
        assert_eq!(rejected.status, CommandStatus::Failed as i32);
        assert_eq!(
            rejected.error.as_ref().map(|error| error.code.as_str()),
            Some("status_query_invalid")
        );
    }
}
