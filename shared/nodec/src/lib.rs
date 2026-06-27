use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base::tokio::sync::mpsc;
use base::tokio::task::JoinHandle;
use base::tokio_util::sync::CancellationToken;
use base_rpc::{RpcChannelConfig, connect_channel};
use gmv_protocol::guard::v1::guard_node_control_client::GuardNodeControlClient;
use gmv_protocol::guard::v1::{
    HostMetrics, NodeHealth, NodeHeartbeat, NodeToGuardMessage, RegisterNodeRequest,
    node_to_guard_message,
};
use sys_metrics::HostMetricsCollector;
use tokio_stream::wrappers::ReceiverStream;

pub type BusinessMetrics = Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>;

#[derive(Clone)]
pub struct NodeReporterConfig {
    pub channel: RpcChannelConfig,
    pub register: RegisterNodeRequest,
    pub health: NodeHealth,
    pub business_metrics: BusinessMetrics,
    pub reconnect_delay: Duration,
}

impl NodeReporterConfig {
    pub fn new(channel: RpcChannelConfig, register: RegisterNodeRequest) -> Self {
        Self {
            channel,
            register,
            health: NodeHealth::Ready,
            business_metrics: Arc::new(HashMap::new),
            reconnect_delay: Duration::from_secs(3),
        }
    }
}

#[must_use]
pub fn generate_instance_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub struct NodeReporter;

impl NodeReporter {
    pub fn spawn(config: NodeReporterConfig, cancel: CancellationToken) -> JoinHandle<()> {
        base::tokio::spawn(async move {
            let mut sequence = 0u64;
            let mut collector = HostMetricsCollector::new();
            while !cancel.is_cancelled() {
                let result = run_connection(&config, &cancel, &mut collector, &mut sequence).await;
                if cancel.is_cancelled() {
                    break;
                }
                if let Err(error) = result {
                    base::log::warn!("Guard node reporter disconnected: {error}");
                }
                base::tokio::select! {
                    _ = base::tokio::time::sleep(config.reconnect_delay) => {}
                    _ = cancel.cancelled() => break,
                }
            }
        })
    }
}

async fn run_connection(
    config: &NodeReporterConfig,
    cancel: &CancellationToken,
    collector: &mut HostMetricsCollector,
    sequence: &mut u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let channel = connect_channel(&config.channel).await?;
    let mut client = GuardNodeControlClient::new(channel);
    let mut register = config.register.clone();
    register.host_metrics = collector.sample().ok().map(host_metrics);
    let response = client.register_node(register.clone()).await?.into_inner();
    let interval_ms = response.heartbeat_interval_ms.max(1_000);
    let (tx, rx) = mpsc::channel(16);
    let mut output = client
        .open_control_stream(ReceiverStream::new(rx))
        .await?
        .into_inner();
    let identity = register.identity;
    let mut interval = base::tokio::time::interval(Duration::from_millis(interval_ms));
    loop {
        base::tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = interval.tick() => {
                *sequence = sequence.saturating_add(1);
                let message = NodeToGuardMessage {
                    identity: identity.clone(),
                    sequence: *sequence,
                    sent_at_epoch_ms: now_ms(),
                    payload: Some(node_to_guard_message::Payload::Heartbeat(NodeHeartbeat {
                        health: config.health as i32,
                        metrics: (config.business_metrics)(),
                        host_metrics: collector.sample().ok().map(host_metrics),
                    })),
                };
                tx.send(message).await?;
            }
            response = output.message() => {
                if response?.is_none() { return Err("Guard control stream closed".into()); }
            }
        }
    }
}

#[must_use]
pub fn host_metrics(value: sys_metrics::HostMetrics) -> HostMetrics {
    HostMetrics {
        cpu_usage_percent: value.cpu_usage_percent,
        load_average_1m: value.load_average_1m,
        load_average_5m: value.load_average_5m,
        load_average_15m: value.load_average_15m,
        memory_total_bytes: value.memory_total_bytes,
        memory_used_bytes: value.memory_used_bytes,
        swap_total_bytes: value.swap_total_bytes,
        swap_used_bytes: value.swap_used_bytes,
        disk_read_bytes_per_sec: value.disk_read_bytes_per_sec,
        disk_write_bytes_per_sec: value.disk_write_bytes_per_sec,
        network_receive_bytes_per_sec: value.network_receive_bytes_per_sec,
        network_transmit_bytes_per_sec: value.network_transmit_bytes_per_sec,
        process_resident_memory_bytes: value.process_resident_memory_bytes,
        process_threads: value.process_threads,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_host_metrics_without_unit_changes() {
        let value = host_metrics(sys_metrics::HostMetrics {
            cpu_usage_percent: 12.5,
            memory_total_bytes: 100,
            process_threads: 4,
            ..Default::default()
        });
        assert_eq!(value.cpu_usage_percent, 12.5);
        assert_eq!(value.memory_total_bytes, 100);
        assert_eq!(value.process_threads, 4);
    }
}
