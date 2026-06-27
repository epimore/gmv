use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use avai::guard_integration::{AvaiControlAdapter, AvaiControlRpc, AvaiGuardNode};
use base::cfg_lib::conf;
use base::serde::Deserialize;
use base::tokio_util::sync::CancellationToken;
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::avai::v1::avai_control_server::AvaiControlServer;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard")]
struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server")]
struct ServerConf {
    #[serde(default = "default_node_id")]
    node_id: String,
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_grpc_port")]
    grpc_port: u16,
    #[serde(default = "default_capabilities")]
    capabilities: Vec<String>,
}

fn default_guard_endpoint() -> String {
    std::env::var("GMV_GUARD_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:18080".to_string())
}

fn default_node_id() -> String {
    std::env::var("GMV_AVAI_NODE_ID").unwrap_or_else(|_| "avai-node-1".to_string())
}

fn default_host() -> String {
    std::env::var("GMV_AVAI_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn default_grpc_port() -> u16 {
    std::env::var("GMV_AVAI_GRPC_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(19080)
}

fn default_capabilities() -> Vec<String> {
    vec!["ai.vehicle".to_string()]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let guard = GuardConf::conf();
        let server = ServerConf::conf();
        let capabilities = server.capabilities.clone();
        let mut node = AvaiGuardNode::new(
            server.node_id,
            generate_instance_id(),
            server.host,
            guard.endpoint,
            u32::from(server.grpc_port),
            capabilities.clone(),
        );
        node.started_at_epoch_ms = now_epoch_ms();
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
        let address: SocketAddr = format!("0.0.0.0:{}", server.grpc_port).parse()?;
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
