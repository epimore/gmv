use std::net::SocketAddr;
use std::pin::Pin;

use base::futures::Stream;
use base::tokio::sync::mpsc;
use gmv_protocol::common::v1::{NodeIdentity as ProtoIdentity, NodeKind as ProtoNodeKind};
use gmv_protocol::guard::v1::guard_node_control_server::{
    GuardNodeControl, GuardNodeControlServer,
};
use gmv_protocol::guard::v1::{
    GuardToNodeMessage, HostMetrics, NodeHealth, NodeHeartbeat, NodeToGuardMessage,
    RegisterDecision as ProtoRegisterDecision, RegisterNodeRequest, RegisterNodeResponse,
    StreamAck, guard_to_node_message, node_to_guard_message,
};
use tonic::{Request, Response, Status, Streaming};

use crate::core::{GuardError, HealthState, NodeIdentity, NodeKind};
use crate::registry::{HeartbeatReport, RegisterDecision, RegisterRequest, RegistryService};
use crate::store::model::HostMetricsRecord;

#[derive(Debug, Clone)]
pub struct NodeRpcConfig {
    pub bind_addr: SocketAddr,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct GuardNodeRpc {
    registry: RegistryService,
    heartbeat_interval_ms: u64,
    heartbeat_timeout_ms: u64,
}

impl GuardNodeRpc {
    pub fn new(
        registry: RegistryService,
        heartbeat_interval_ms: u64,
        heartbeat_timeout_ms: u64,
    ) -> Self {
        Self {
            registry,
            heartbeat_interval_ms,
            heartbeat_timeout_ms,
        }
    }
}

type ControlStream = Pin<Box<dyn Stream<Item = Result<GuardToNodeMessage, Status>> + Send>>;

#[tonic::async_trait]
impl GuardNodeControl for GuardNodeRpc {
    async fn register_node(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<RegisterNodeResponse>, Status> {
        let request = request.into_inner();
        let identity = identity(request.identity)?;
        let decision = self
            .registry
            .register(RegisterRequest {
                identity,
                capabilities: request.capabilities,
                capacity: request.capacity.max(1),
                host_metrics: host_metrics(request.host_metrics),
                zone: (!request.zone.is_empty()).then_some(request.zone),
                now_ms: now_ms(),
                takeover: request.takeover,
            })
            .map_err(status)?;
        Ok(Response::new(RegisterNodeResponse {
            decision: match decision {
                RegisterDecision::Accepted => ProtoRegisterDecision::Accepted as i32,
                RegisterDecision::Reconnected => ProtoRegisterDecision::Reconnected as i32,
                RegisterDecision::SupersededOldInstance => {
                    ProtoRegisterDecision::SupersededOldInstance as i32
                }
            },
            guard_epoch_ms: now_ms(),
            heartbeat_interval_ms: self.heartbeat_interval_ms,
            heartbeat_timeout_ms: self.heartbeat_timeout_ms,
            message: String::new(),
        }))
    }

    type OpenControlStreamStream = ControlStream;

    async fn open_control_stream(
        &self,
        request: Request<Streaming<NodeToGuardMessage>>,
    ) -> Result<Response<Self::OpenControlStreamStream>, Status> {
        let mut input = request.into_inner();
        let registry = self.registry.clone();
        let (tx, rx) = mpsc::channel(32);
        base::tokio::spawn(async move {
            while let Ok(Some(message)) = input.message().await {
                let sequence = message.sequence;
                let result = match message.payload {
                    Some(node_to_guard_message::Payload::Heartbeat(heartbeat)) => apply_heartbeat(
                        &registry,
                        message.identity,
                        sequence,
                        message.sent_at_epoch_ms,
                        heartbeat,
                    ),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    let _ = tx.send(Err(status(error))).await;
                    return;
                }
                let ack = GuardToNodeMessage {
                    message_id: format!("ack-{sequence}"),
                    sent_at_epoch_ms: now_ms(),
                    payload: Some(guard_to_node_message::Payload::Ack(StreamAck {
                        received_sequence: sequence,
                    })),
                };
                if tx.send(Ok(ack)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}

pub async fn serve(
    config: NodeRpcConfig,
    registry: RegistryService,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = GuardNodeRpc::new(
        registry,
        config.heartbeat_interval_ms,
        config.heartbeat_timeout_ms,
    );
    tonic::transport::Server::builder()
        .add_service(GuardNodeControlServer::new(service))
        .serve(config.bind_addr)
        .await?;
    Ok(())
}

fn apply_heartbeat(
    registry: &RegistryService,
    identity_value: Option<ProtoIdentity>,
    sequence: u64,
    _sent_at_epoch_ms: i64,
    heartbeat: NodeHeartbeat,
) -> Result<(), GuardError> {
    registry.heartbeat(HeartbeatReport {
        identity: identity(identity_value)
            .map_err(|error| GuardError::InvalidIdentity(error.message().to_string()))?,
        health: health(heartbeat.health),
        sequence,
        now_ms: now_ms(),
        host_metrics: host_metrics(heartbeat.host_metrics),
        business_metrics: heartbeat.metrics,
    })
}

fn identity(value: Option<ProtoIdentity>) -> Result<NodeIdentity, Status> {
    let value = value.ok_or_else(|| Status::invalid_argument("identity is required"))?;
    let kind = match ProtoNodeKind::try_from(value.kind).ok() {
        Some(ProtoNodeKind::Session) => NodeKind::Session,
        Some(ProtoNodeKind::Stream) => NodeKind::Stream,
        Some(ProtoNodeKind::Avai) => NodeKind::Avai,
        _ => return Err(Status::invalid_argument("node kind is required")),
    };
    Ok(NodeIdentity::new(value.node_id, value.instance_id, kind))
}

fn health(value: i32) -> HealthState {
    match NodeHealth::try_from(value).unwrap_or(NodeHealth::Unspecified) {
        NodeHealth::Starting => HealthState::Starting,
        NodeHealth::Ready => HealthState::Ready,
        NodeHealth::Degraded => HealthState::Degraded,
        NodeHealth::Draining => HealthState::Draining,
        NodeHealth::Offline | NodeHealth::Unspecified => HealthState::Offline,
    }
}

fn host_metrics(value: Option<HostMetrics>) -> HostMetricsRecord {
    value.map_or_else(HostMetricsRecord::default, |value| HostMetricsRecord {
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
    })
}

fn status(error: GuardError) -> Status {
    match error {
        GuardError::Conflict(message) => Status::already_exists(message),
        GuardError::StaleInstance(message) => Status::failed_precondition(message),
        GuardError::NotFound(message) => Status::not_found(message),
        other => Status::invalid_argument(other.to_string()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
