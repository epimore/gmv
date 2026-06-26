use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use avai::guard_integration::{AvaiControlAdapter, AvaiControlRpc, AvaiGuardNode};
use base::tokio_util::sync::CancellationToken;
use gmv_node_client::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::avai::v1::avai_control_server::AvaiControlServer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let node_id =
            std::env::var("GMV_AVAI_NODE_ID").unwrap_or_else(|_| "avai-node-1".to_string());
        let grpc_port = std::env::var("GMV_AVAI_GRPC_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(19080);
        let capabilities = vec!["ai.vehicle".to_string()];
        let mut node = AvaiGuardNode::new(
            node_id,
            generate_instance_id(),
            u32::from(grpc_port),
            capabilities.clone(),
        );
        node.started_at_epoch_ms = now_epoch_ms();
        if let Ok(endpoint) = std::env::var("GMV_GUARD_ENDPOINT") {
            node.guard_channel.endpoint = endpoint;
        }
        let adapter = AvaiControlAdapter::new(node.identity.clone(), capabilities);
        let snapshot = adapter.resource_snapshot();
        let rpc = AvaiControlRpc::new(adapter);
        let metrics_rpc = rpc.clone();
        let mut reporter =
            NodeReporterConfig::new(node.guard_channel.clone(), node.register_request(snapshot));
        reporter.business_metrics = Arc::new(move || {
            HashMap::from([(
                "running_tasks".to_string(),
                metrics_rpc.running_task_count().to_string(),
            )])
        });
        let cancel = CancellationToken::new();
        let reporter_task = NodeReporter::spawn(reporter, cancel.clone());
        let address: SocketAddr = format!("127.0.0.1:{grpc_port}").parse()?;
        let shutdown = cancel.clone();
        let server = tonic::transport::Server::builder()
            .add_service(AvaiControlServer::new(rpc))
            .serve_with_shutdown(address, async move { shutdown.cancelled().await });
        base::tokio::select! {
            result = server => result?,
            _ = base::tokio::signal::ctrl_c() => cancel.cancel(),
        }
        cancel.cancel();
        let _ = reporter_task.await;
        Ok(())
    })
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
