use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::pin::Pin;

use base::futures::Stream;
use base::tokio::sync::mpsc;
use gmv_protocol::common::v1::{
    Endpoint as ProtoEndpoint, EndpointMode as ProtoEndpointMode, NodeIdentity as ProtoIdentity,
    NodeKind as ProtoNodeKind,
};
use gmv_protocol::guard::v1::guard_control_server::GuardControlServer;
use gmv_protocol::guard::v1::guard_node_control_server::{
    GuardNodeControl, GuardNodeControlServer,
};
use gmv_protocol::guard::v1::{
    EventPriority, GuardToNodeMessage, HostMetrics, NodeHealth, NodeHeartbeat,
    NodeResourceSnapshot, NodeToGuardMessage, RegisterDecision as ProtoRegisterDecision,
    RegisterNodeRequest, RegisterNodeResponse, StreamAck, guard_to_node_message,
    node_to_guard_message,
};
use tonic::{Request, Response, Status, Streaming};

use crate::auth::AuthState;
use crate::core::{GuardError, HealthState, NodeIdentity, NodeKind, RouteState};
use crate::registry::{HeartbeatReport, RegisterDecision, RegisterRequest, RegistryService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::runtime::event_forwarder::EventForwarder;
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointModeRecord, EndpointRecord, EventRecord, HostMetricsRecord};

#[derive(Debug, Clone)]
pub struct NodeRpcConfig {
    pub bind_addr: SocketAddr,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub tls: Option<NodeRpcTlsConfig>,
}

#[derive(Debug, Clone)]
pub struct NodeRpcTlsConfig {
    pub certificate_path: PathBuf,
    pub private_key_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct GuardNodeRpc {
    registry: RegistryService,
    routes: RouteService,
    store: InMemoryGuardStore,
    forwarder: Option<EventForwarder>,
    heartbeat_interval_ms: u64,
    heartbeat_timeout_ms: u64,
}

impl GuardNodeRpc {
    pub fn new(
        registry: RegistryService,
        store: InMemoryGuardStore,
        heartbeat_interval_ms: u64,
        heartbeat_timeout_ms: u64,
        forwarder: Option<EventForwarder>,
    ) -> Self {
        Self {
            registry,
            routes: RouteService::new(store.clone()),
            store,
            forwarder,
            heartbeat_interval_ms,
            heartbeat_timeout_ms,
        }
    }
}

type ControlStream = Pin<Box<dyn Stream<Item = Result<GuardToNodeMessage, Status>> + Send>>;

#[derive(Debug, Clone)]
struct ControlStreamOwner {
    identity: NodeIdentity,
    generation: u64,
}

#[derive(Debug)]
enum ControlStreamEnd {
    NormalEof,
    RemoteEof,
    TransportError(Status),
    OutputReceiverDropped,
    ApplicationError(String),
}

#[tonic::async_trait]
impl GuardNodeControl for GuardNodeRpc {
    async fn register_node(
        &self,
        request: Request<RegisterNodeRequest>,
    ) -> Result<Response<RegisterNodeResponse>, Status> {
        let request = request.into_inner();
        base::log::debug!(
            "guard_node.register_node, req: identity={:?}, software_version={}, started_at_epoch_ms={}, endpoints={:?}, capabilities={:?}, zone={}, takeover={}, config={:?}, has_snapshot={}, has_host_metrics={}",
            request.identity,
            request.software_version,
            request.started_at_epoch_ms,
            request.endpoints,
            request.capabilities,
            request.zone,
            request.takeover,
            request.config,
            request.startup_snapshot.is_some(),
            request.host_metrics.is_some()
        );
        let identity = identity(request.identity)?;
        let startup_snapshot = request.startup_snapshot.clone();
        let decision = self
            .registry
            .register(RegisterRequest {
                identity: identity.clone(),
                capabilities: request.capabilities,
                endpoints: endpoint_records(request.endpoints),
                host_metrics: host_metrics(request.host_metrics),
                zone: (!request.zone.is_empty()).then_some(request.zone),
                now_ms: now_ms(),
                takeover: request.takeover,
                config: request.config,
            })
            .map_err(status)?;
        if let Some(snapshot) = startup_snapshot {
            let generation = self
                .store
                .get_node(&identity.node_id)
                .map_or(1, |node| node.generation);
            apply_snapshot(&self.routes, identity.clone(), generation, 1, snapshot)
                .map_err(status)?;
        }
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
        base::log::debug!("guard_node.open_control_stream, req:<stream>");
        let mut input = request.into_inner();
        let registry = self.registry.clone();
        let routes = self.routes.clone();
        let store = self.store.clone();
        let forwarder = self.forwarder.clone();
        let (tx, rx) = mpsc::channel(32);
        base::tokio::spawn(async move {
            let mut stream_owner: Option<ControlStreamOwner> = None;
            let end = loop {
                let message = match input.message().await {
                    Ok(Some(message)) => message,
                    Ok(None) if stream_owner.is_none() => break ControlStreamEnd::NormalEof,
                    Ok(None) => break ControlStreamEnd::RemoteEof,
                    Err(error) => break ControlStreamEnd::TransportError(error),
                };
                let message_owner = match control_stream_owner(&store, message.identity.as_ref()) {
                    Ok(owner) => owner,
                    Err(error) => {
                        let error_message = error.to_string();
                        if tx.send(Err(status(error))).await.is_err() {
                            break ControlStreamEnd::OutputReceiverDropped;
                        }
                        break ControlStreamEnd::ApplicationError(error_message);
                    }
                };
                if let Some(owner) = stream_owner.as_ref()
                    && (owner.identity != message_owner.identity
                        || owner.generation != message_owner.generation)
                {
                    let error = GuardError::StaleInstance(
                        "control stream identity or generation changed".to_string(),
                    );
                    let error_message = error.to_string();
                    if tx.send(Err(status(error))).await.is_err() {
                        break ControlStreamEnd::OutputReceiverDropped;
                    }
                    break ControlStreamEnd::ApplicationError(error_message);
                }
                stream_owner.get_or_insert(message_owner.clone());
                let sequence = message.sequence;
                match &message.payload {
                    Some(node_to_guard_message::Payload::Heartbeat(_)) => {
                        base::log::trace!(
                            "guard_node.open_control_stream message, req: identity={:?}, sequence={}, sent_at_epoch_ms={}, payload=heartbeat",
                            message.identity,
                            sequence,
                            message.sent_at_epoch_ms,
                        );
                    }
                    Some(node_to_guard_message::Payload::Snapshot(snapshot)) => {
                        base::log::debug!(
                            "guard_node.open_control_stream message, req: identity={:?}, sequence={}, sent_at_epoch_ms={}, payload=snapshot, resources={}, full={}",
                            message.identity,
                            sequence,
                            message.sent_at_epoch_ms,
                            snapshot.resources.len(),
                            snapshot.full
                        );
                    }
                    Some(node_to_guard_message::Payload::Event(event)) => {
                        base::log::debug!(
                            "guard_node.open_control_stream message, req: identity={:?}, sequence={}, sent_at_epoch_ms={}, payload=event, event_id={}, topic={}, payload_bytes={}",
                            message.identity,
                            sequence,
                            message.sent_at_epoch_ms,
                            event.event_id,
                            event.topic,
                            event.payload.len()
                        );
                    }
                    Some(_) => base::log::debug!(
                        "guard_node.open_control_stream message, req: identity={:?}, sequence={}, sent_at_epoch_ms={}, payload=other",
                        message.identity,
                        sequence,
                        message.sent_at_epoch_ms,
                    ),
                    None => base::log::debug!(
                        "guard_node.open_control_stream message, req: identity={:?}, sequence={}, sent_at_epoch_ms={}, payload=none",
                        message.identity,
                        sequence,
                        message.sent_at_epoch_ms,
                    ),
                }
                let result = match message.payload {
                    Some(node_to_guard_message::Payload::Heartbeat(heartbeat)) => apply_heartbeat(
                        &registry,
                        message.identity,
                        sequence,
                        message.sent_at_epoch_ms,
                        heartbeat,
                    ),
                    Some(node_to_guard_message::Payload::Snapshot(snapshot)) => apply_snapshot(
                        &routes,
                        message_owner.identity,
                        message_owner.generation,
                        sequence,
                        snapshot,
                    ),
                    Some(node_to_guard_message::Payload::Event(event)) => {
                        apply_event(&store, forwarder.as_ref(), event).await
                    }
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    let error_message = error.to_string();
                    if tx.send(Err(status(error))).await.is_err() {
                        break ControlStreamEnd::OutputReceiverDropped;
                    }
                    break ControlStreamEnd::ApplicationError(error_message);
                }
                let ack = GuardToNodeMessage {
                    message_id: format!("ack-{sequence}"),
                    sent_at_epoch_ms: now_ms(),
                    payload: Some(guard_to_node_message::Payload::Ack(StreamAck {
                        received_sequence: sequence,
                    })),
                };
                if tx.send(Ok(ack)).await.is_err() {
                    break ControlStreamEnd::OutputReceiverDropped;
                }
            };
            finish_control_stream(&registry, &store, stream_owner.as_ref(), end);
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}

pub async fn serve(
    config: NodeRpcConfig,
    listener: StdTcpListener,
    registry: RegistryService,
    store: InMemoryGuardStore,
    auth: AuthState,
    forwarder: Option<EventForwarder>,
) -> Result<(), Box<dyn std::error::Error>> {
    base::log::debug!(
        "guard rpc service inbound: bind_addr={}, tls={}",
        config.bind_addr,
        config.tls.is_some()
    );
    let node_service = GuardNodeRpc::new(
        registry,
        store.clone(),
        config.heartbeat_interval_ms,
        config.heartbeat_timeout_ms,
        forwarder,
    );
    let control_service = crate::runtime::control_rpc::GuardControlRpc::with_auth(store, auth);
    let mut server_config = base_rpc::RpcServerConfig::default();
    if let Some(tls) = config.tls {
        server_config.tls = Some(base_rpc::load_server_tls_from_files(
            &base_rpc::TlsFileConfig {
                certificate_path: Some(tls.certificate_path),
                private_key_path: Some(tls.private_key_path),
                ..base_rpc::TlsFileConfig::default()
            },
        )?);
    }
    let incoming = base_rpc::tcp_incoming_from_std(listener)?;
    base_rpc::build_server(&server_config)?
        .add_service(GuardNodeControlServer::new(node_service))
        .add_service(GuardControlServer::new(control_service))
        .serve_with_incoming(incoming)
        .await?;
    base::log::debug!("guard rpc service outbound: bind_addr={}", config.bind_addr);
    Ok(())
}

fn endpoint_records(endpoints: Vec<ProtoEndpoint>) -> Vec<EndpointRecord> {
    endpoints
        .into_iter()
        .map(|endpoint| EndpointRecord {
            name: endpoint.name,
            scheme: endpoint.scheme,
            host: endpoint.host,
            port: endpoint.port,
            mode: match ProtoEndpointMode::try_from(endpoint.mode)
                .unwrap_or(ProtoEndpointMode::Unspecified)
            {
                ProtoEndpointMode::Multi => EndpointModeRecord::Multi,
                ProtoEndpointMode::Single | ProtoEndpointMode::Unspecified => {
                    EndpointModeRecord::Single
                }
            },
            labels: endpoint.labels,
        })
        .collect()
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

fn control_stream_owner(
    store: &InMemoryGuardStore,
    value: Option<&ProtoIdentity>,
) -> Result<ControlStreamOwner, GuardError> {
    let identity = identity(value.cloned())
        .map_err(|error| GuardError::InvalidIdentity(error.message().to_string()))?;
    let node = store
        .get_node(&identity.node_id)
        .ok_or_else(|| GuardError::NotFound(format!("node {}", identity.node_id)))?;
    if node.identity != identity {
        return Err(GuardError::StaleInstance(format!(
            "node {} stale instance {} current {}",
            identity.node_id, identity.instance_id, node.identity.instance_id
        )));
    }
    Ok(ControlStreamOwner {
        identity,
        generation: node.generation,
    })
}

fn finish_control_stream(
    registry: &RegistryService,
    store: &InMemoryGuardStore,
    owner: Option<&ControlStreamOwner>,
    end: ControlStreamEnd,
) {
    let Some(owner) = owner else {
        base::log::debug!(
            "guard control stream ended: outcome=normal_eof, reason=no_authenticated_message"
        );
        return;
    };
    let current = store
        .get_node(&owner.identity.node_id)
        .is_some_and(|node| node.identity == owner.identity && node.generation == owner.generation);
    if !current {
        base::log::debug!(
            "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=normal_eof, reason=stale_generation",
            owner.identity.node_id,
            owner.identity.instance_id,
            owner.generation
        );
        return;
    }
    match end {
        ControlStreamEnd::NormalEof => {
            base::log::debug!(
                "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=normal_eof",
                owner.identity.node_id,
                owner.identity.instance_id,
                owner.generation
            );
        }
        ControlStreamEnd::RemoteEof => {
            let disconnected = registry.disconnect_if_current(&owner.identity, owner.generation);
            base::log::warn!(
                "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=remote_eof, reason=unexpected_remote_eof, disconnected={}",
                owner.identity.node_id,
                owner.identity.instance_id,
                owner.generation,
                disconnected
            );
        }
        ControlStreamEnd::TransportError(error) => {
            let disconnected = registry.disconnect_if_current(&owner.identity, owner.generation);
            base::log::warn!(
                "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=transport_error, tonic_code={:?}, reason={}, disconnected={}",
                owner.identity.node_id,
                owner.identity.instance_id,
                owner.generation,
                error.code(),
                error.message(),
                disconnected
            );
        }
        ControlStreamEnd::OutputReceiverDropped => {
            let disconnected = registry.disconnect_if_current(&owner.identity, owner.generation);
            base::log::warn!(
                "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=output_receiver_dropped, disconnected={}",
                owner.identity.node_id,
                owner.identity.instance_id,
                owner.generation,
                disconnected
            );
        }
        ControlStreamEnd::ApplicationError(error) => {
            let disconnected = registry.disconnect_if_current(&owner.identity, owner.generation);
            base::log::warn!(
                "guard control stream ended: node_id={}, instance_id={}, generation={}, outcome=application_error, reason={}, disconnected={}",
                owner.identity.node_id,
                owner.identity.instance_id,
                owner.generation,
                error,
                disconnected
            );
        }
    }
}

fn apply_snapshot(
    routes: &RouteService,
    owner: NodeIdentity,
    generation: u64,
    sequence: u64,
    snapshot: NodeResourceSnapshot,
) -> Result<(), GuardError> {
    routes.apply_snapshot(ResourceSnapshot {
        owner,
        generation,
        sequence,
        resources: snapshot
            .resources
            .into_iter()
            .filter_map(|resource| {
                let resource_ref = resource.resource?;
                Some(SnapshotResource {
                    resource_id: resource_ref.resource_id,
                    route_id: resource.labels.get("route_id").cloned(),
                })
            })
            .collect(),
    })?;
    Ok(())
}

async fn apply_event(
    store: &InMemoryGuardStore,
    forwarder: Option<&EventForwarder>,
    event: gmv_protocol::guard::v1::NodeEvent,
) -> Result<(), GuardError> {
    if event.event_id.is_empty() || event.topic.is_empty() {
        return Err(GuardError::InvalidConfig(
            "node event_id and topic are required".to_string(),
        ));
    }
    let event_id = event.event_id;
    let topic = event.topic;
    let priority = event_priority(event.priority);
    let payload = event.payload;
    let payload_bytes = payload.len();
    let inserted = store.insert_event_once(EventRecord {
        event_id: event_id.clone(),
        topic: topic.clone(),
        priority,
        payload: payload.clone(),
    })?;
    if inserted && topic == "session.playback_presence_terminal" {
        apply_playback_presence_terminal(store, &payload)?;
    }
    if inserted
        && let Some(forwarder) = forwarder
        && let Err(error) = forwarder
            .forward(event_id.clone(), topic.clone(), payload)
            .await
    {
        store.remove_event(&event_id);
        return Err(error);
    }
    if inserted {
        base::log::info!(
            "guard node event stored: event_id={}, topic={}, priority={}, payload_bytes={}",
            event_id,
            topic,
            priority,
            payload_bytes
        );
    } else {
        base::log::debug!(
            "guard node event deduplicated: event_id={}, topic={}, priority={}, payload_bytes={}",
            event_id,
            topic,
            priority,
            payload_bytes
        );
    }
    Ok(())
}

#[derive(base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackPresenceTerminalEvent {
    stream_id: String,
    subscription_id: String,
    #[serde(default)]
    stream_stopped: bool,
}

fn apply_playback_presence_terminal(
    store: &InMemoryGuardStore,
    payload: &[u8],
) -> Result<(), GuardError> {
    let event: PlaybackPresenceTerminalEvent =
        base::serde_json::from_slice(payload).map_err(|err| {
            GuardError::InvalidConfig(format!(
                "decode playback presence terminal event failed: {err}"
            ))
        })?;
    store.revoke_playback_tickets_for_subscription(&event.stream_id, &event.subscription_id);
    if event.stream_stopped {
        store.revoke_playback_tickets_for_stream(&event.stream_id);
        for route in store.routes().into_iter().filter(|route| {
            route.resource_id == event.stream_id && route.state != RouteState::Closed
        }) {
            let mut route = route;
            route.state = RouteState::Closed;
            store.upsert_route(route);
        }
    }
    Ok(())
}

fn event_priority(value: i32) -> u8 {
    match EventPriority::try_from(value).unwrap_or(EventPriority::Unspecified) {
        EventPriority::P0 => 1,
        EventPriority::P1 => 2,
        EventPriority::P2 => 3,
        EventPriority::P3 | EventPriority::Unspecified => 4,
    }
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
