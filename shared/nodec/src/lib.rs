use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::tokio::sync::mpsc;
use base::tokio::task::JoinHandle;
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;

use base_rpc::{RpcChannelConfig, connect_channel};
use gmv_protocol::guard::v1::guard_node_control_client::GuardNodeControlClient;
use gmv_protocol::guard::v1::{
    HostMetrics, NodeEvent, NodeHealth, NodeHeartbeat, NodeResourceSnapshot, NodeToGuardMessage,
    RegisterNodeRequest, node_to_guard_message,
};
use sys_metrics::HostMetricsCollector;
use tokio_stream::wrappers::ReceiverStream;

pub mod error;
pub mod error_code;

pub type BusinessMetrics = Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>;
pub type ResourceSnapshotFuture = Pin<Box<dyn Future<Output = NodeResourceSnapshot> + Send>>;
pub type ResourceSnapshotProvider = Arc<dyn Fn() -> ResourceSnapshotFuture + Send + Sync>;

#[derive(Clone)]
pub struct NodeReporterConfig {
    pub channel: RpcChannelConfig,
    pub register: RegisterNodeRequest,
    pub health: NodeHealth,
    pub business_metrics: BusinessMetrics,
    pub resource_snapshot: Option<ResourceSnapshotProvider>,
    pub reconnect_delay: Duration,
}

impl NodeReporterConfig {
    pub fn new(channel: RpcChannelConfig, register: RegisterNodeRequest) -> Self {
        Self {
            channel,
            register,
            health: NodeHealth::Ready,
            business_metrics: Arc::new(HashMap::new),
            resource_snapshot: None,
            reconnect_delay: Duration::from_secs(3),
        }
    }
}

#[must_use]
pub fn generate_instance_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[derive(Clone)]
pub struct NodeEventSender {
    tx: mpsc::Sender<NodeEvent>,
}

impl NodeEventSender {
    pub fn try_send(&self, event: NodeEvent) -> Result<(), mpsc::error::TrySendError<NodeEvent>> {
        let event_id = event.event_id.clone();
        let topic = event.topic.clone();
        let payload_bytes = event.payload.len();
        self.tx.try_send(event)?;
        base::log::debug!(
            "guard event enqueue: event_id={}, topic={}, payload_bytes={}",
            event_id,
            topic,
            payload_bytes
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlStreamEnd {
    LocalCancelled,
    RemoteEof,
    TransportError(tonic::Code),
    OutputReceiverDropped,
}

pub struct NodeReporter;

impl NodeReporter {
    pub fn spawn(config: NodeReporterConfig, cancel: CancellationToken) -> JoinHandle<()> {
        let (handle, _) = Self::spawn_with_events(config, cancel);
        handle
    }

    pub fn spawn_with_events(
        config: NodeReporterConfig,
        cancel: CancellationToken,
    ) -> (JoinHandle<()>, NodeEventSender) {
        let (event_tx, event_rx) = mpsc::channel(128);
        let sender = NodeEventSender { tx: event_tx };
        let handle = base::tokio::spawn(run_reporter(config, cancel, event_rx));
        (handle, sender)
    }

    pub fn spawn_managed(
        runtime: &GlobalRuntime,
        config: NodeReporterConfig,
        cancel: CancellationToken,
    ) -> base::exception::GlobalResult<()> {
        let (event_tx, event_rx) = mpsc::channel(128);
        drop(event_tx);
        drop(runtime.spawn("node-reporter", run_reporter(config, cancel, event_rx))?);
        Ok(())
    }

    pub fn spawn_managed_with_events(
        runtime: &GlobalRuntime,
        config: NodeReporterConfig,
        cancel: CancellationToken,
    ) -> base::exception::GlobalResult<NodeEventSender> {
        let (event_tx, event_rx) = mpsc::channel(128);
        let sender = NodeEventSender { tx: event_tx };
        drop(runtime.spawn("node-reporter", run_reporter(config, cancel, event_rx))?);
        Ok(sender)
    }
}

async fn run_reporter(
    config: NodeReporterConfig,
    cancel: CancellationToken,
    mut event_rx: mpsc::Receiver<NodeEvent>,
) {
    let mut sequence = 0u64;
    let mut collector = HostMetricsCollector::new();
    let mut connection_episode = FailureEpisode::default();
    while !cancel.is_cancelled() {
        let result = run_connection(
            &config,
            &cancel,
            &mut collector,
            &mut sequence,
            &mut event_rx,
            &mut connection_episode,
        )
        .await;
        if cancel.is_cancelled() {
            base::log::trace!("node reporter control stream ended: outcome=local_cancelled");
            break;
        }
        match result {
            Ok(ControlStreamEnd::LocalCancelled) => {
                base::log::trace!("node reporter control stream ended: outcome=local_cancelled");
                break;
            }
            Ok(ControlStreamEnd::RemoteEof) => {
                record_connection_failure(&mut connection_episode, "remote_eof", None, None);
            }
            Ok(ControlStreamEnd::TransportError(code)) => {
                record_connection_failure(
                    &mut connection_episode,
                    "transport_error",
                    Some(code),
                    None,
                );
            }
            Ok(ControlStreamEnd::OutputReceiverDropped) => {
                record_connection_failure(
                    &mut connection_episode,
                    "output_receiver_dropped",
                    None,
                    None,
                );
            }
            Err(error) => {
                record_connection_failure(
                    &mut connection_episode,
                    "connection_error",
                    None,
                    Some(error.as_ref()),
                );
            }
        }
        base::tokio::select! {
            _ = base::tokio::time::sleep(config.reconnect_delay) => {}
            _ = cancel.cancelled() => break,
        }
    }
}

async fn run_connection(
    config: &NodeReporterConfig,
    cancel: &CancellationToken,
    collector: &mut HostMetricsCollector,
    sequence: &mut u64,
    event_rx: &mut mpsc::Receiver<NodeEvent>,
    connection_episode: &mut FailureEpisode,
) -> Result<ControlStreamEnd, Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    base::log::trace!(
        "node reporter rpc client outbound: service=guard_node_control, endpoint={}",
        config.channel.endpoint
    );
    let channel = connect_channel(&config.channel).await?;
    base::log::trace!(
        "node reporter rpc client inbound: service=guard_node_control, endpoint={}, status=ok, elapsed_ms={}",
        config.channel.endpoint,
        started.elapsed().as_millis()
    );
    let mut client = GuardNodeControlClient::new(channel);
    let mut register = config.register.clone();
    if let Some(resource_snapshot) = config.resource_snapshot.as_ref() {
        register.startup_snapshot = Some(resource_snapshot().await);
    }
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
    let mut recovered = false;
    loop {
        base::tokio::select! {
            _ = cancel.cancelled() => return Ok(ControlStreamEnd::LocalCancelled),
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
                if tx.send(message).await.is_err() {
                    return Ok(ControlStreamEnd::OutputReceiverDropped);
                }
            }
            Some(event) = event_rx.recv() => {
                base::log::debug!(
                    "guard event stream send: event_id={}, topic={}, payload_bytes={}",
                    event.event_id,
                    event.topic,
                    event.payload.len()
                );
                *sequence = sequence.saturating_add(1);
                let message = NodeToGuardMessage {
                    identity: identity.clone(),
                    sequence: *sequence,
                    sent_at_epoch_ms: now_ms(),
                    payload: Some(node_to_guard_message::Payload::Event(event)),
                };
                if tx.send(message).await.is_err() {
                    return Ok(ControlStreamEnd::OutputReceiverDropped);
                }
            }
            response = output.message() => {
                match response {
                    Ok(Some(_)) => {
                        if !recovered {
                            record_connection_recovered(connection_episode);
                            recovered = true;
                        }
                    }
                    Ok(None) if cancel.is_cancelled() => {
                        return Ok(ControlStreamEnd::LocalCancelled);
                    }
                    Ok(None) => return Ok(ControlStreamEnd::RemoteEof),
                    Err(_) if cancel.is_cancelled() => {
                        return Ok(ControlStreamEnd::LocalCancelled);
                    }
                    Err(error) => return Ok(ControlStreamEnd::TransportError(error.code())),
                }
            }
        }
    }
}

fn record_connection_failure(
    episode: &mut FailureEpisode,
    reason: &str,
    tonic_code: Option<tonic::Code>,
    error: Option<&dyn std::fmt::Display>,
) {
    if let Some(error) = error {
        base::log::trace!(
            "node reporter connection unavailable: state=down, outcome=failed_attempt, reason={reason}, tonic_code={tonic_code:?}, error={error}"
        );
    } else {
        base::log::trace!(
            "node reporter connection unavailable: state=down, outcome=failed_attempt, reason={reason}, tonic_code={tonic_code:?}"
        );
    }
    match episode.record_failure(Instant::now()) {
        EpisodeDecision::Started => base::log::warn!(
            "node reporter connection state changed: state=down, previous_state=up, reason={reason}, tonic_code={tonic_code:?}"
        ),
        EpisodeDecision::Summary {
            total,
            since_last_summary,
            suppressed,
            duration,
        } => base::log::warn!(
            "node reporter connection unavailable: state=down, outcome=ongoing, reason={reason}, tonic_code={tonic_code:?}, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
            duration.as_millis()
        ),
        EpisodeDecision::Suppressed => {}
        EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
    }
}

fn record_connection_recovered(episode: &mut FailureEpisode) {
    if let EpisodeDecision::Recovered {
        total,
        suppressed,
        duration,
    } = episode.record_success(Instant::now())
    {
        base::log::info!(
            "node reporter connection state changed: state=up, previous_state=down, outcome=recovered, total_failures={total}, suppressed={suppressed}, duration_ms={}",
            duration.as_millis()
        );
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
mod event_sender_tests {
    use super::NodeEventSender;
    use base::tokio::sync::mpsc;
    use gmv_protocol::guard::v1::NodeEvent;

    fn event(id: &str) -> NodeEvent {
        NodeEvent {
            event_id: id.to_string(),
            topic: "test.event".to_string(),
            ..NodeEvent::default()
        }
    }

    #[test]
    fn event_sender_reports_full_and_closed_without_false_success() {
        let (tx, rx) = mpsc::channel(1);
        let sender = NodeEventSender { tx };
        assert!(sender.try_send(event("first")).is_ok());
        assert!(matches!(
            sender.try_send(event("full")),
            Err(mpsc::error::TrySendError::Full(_))
        ));
        drop(rx);
        assert!(matches!(
            sender.try_send(event("closed")),
            Err(mpsc::error::TrySendError::Closed(_))
        ));
    }
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
