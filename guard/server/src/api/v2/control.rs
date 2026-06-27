use gmv_protocol::avai::v1::avai_control_client::AvaiControlClient;
use gmv_protocol::avai::v1::{AiTaskState, CancelTaskRequest, CreateTaskRequest};
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity as ProtoIdentity, NodeKind as ProtoNodeKind,
    OperationRef,
};
use gmv_protocol::session::v1::session_control_client::SessionControlClient;
use gmv_protocol::session::v1::{ControlPtzRequest, DeviceStreamState, StartDeviceStreamRequest};
use gmv_protocol::stream::v1::stream_control_client::StreamControlClient;
use gmv_protocol::stream::v1::{StartReceiveRequest, StopReceiveRequest, StreamState};

use crate::core::{
    ConnectionState, GuardError, GuardResult, LeaseState, NodeIdentity, NodeKind, RouteState,
    SchedulingState,
};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::sim::{SimAiTask, SimAiTaskState, SimStream, SimStreamState};
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointModeRecord, EndpointRecord, NodeRecord, RouteRecord};

#[derive(Debug, Clone)]
pub struct BusinessControl {
    store: InMemoryGuardStore,
}

impl BusinessControl {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub async fn start_live(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<SimStream> {
        self.start_device_stream(DeviceStreamKind::Live, operation_id, device_id, channel_id)
            .await
    }

    pub async fn start_playback(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<SimStream> {
        self.start_device_stream(
            DeviceStreamKind::Playback,
            operation_id,
            device_id,
            channel_id,
        )
        .await
    }

    pub async fn start_download(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<SimStream> {
        self.start_device_stream(
            DeviceStreamKind::Download,
            operation_id,
            device_id,
            channel_id,
        )
        .await
    }

    pub async fn start_talk(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<SimStream> {
        self.start_device_stream(DeviceStreamKind::Talk, operation_id, device_id, channel_id)
            .await
    }

    async fn start_device_stream(
        &self,
        kind: DeviceStreamKind,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<SimStream> {
        let session = self.select_node(NodeKind::Session, kind.session_capability())?;
        let stream = self.select_node(NodeKind::Stream, kind.stream_capability())?;
        let stream_id = format!("{}-{operation_id}", kind.prefix());
        let lease_id = format!("lease-{operation_id}");
        let route_id = format!("route-{operation_id}");
        let operation = OperationRef {
            operation_id: operation_id.to_string(),
            idempotency_key: operation_id.to_string(),
        };
        let allocation =
            AllocationService::new(self.store.clone()).allocate(AllocationRequest {
                request_id: operation_id.to_string(),
                capability: kind.stream_capability().to_string(),
                zone: stream.zone.clone(),
            })?;
        if allocation.owner.node_id != stream.identity.node_id {
            return Err(GuardError::Conflict(
                "selected stream node changed during allocation".to_string(),
            ));
        }
        LeaseService::new(self.store.clone()).allocate(LeaseRequest {
            lease_id: lease_id.clone(),
            route_id: route_id.clone(),
            resource_id: stream_id.clone(),
            idempotency_key: operation_id.to_string(),
            owner: stream.identity.clone(),
            now_ms: now_ms(),
            ttl_ms: 30_000,
        })?;
        RouteService::new(self.store.clone()).create_allocated(RouteRecord {
            route_id: route_id.clone(),
            resource_id: stream_id.clone(),
            node_id: stream.identity.node_id.clone(),
            instance_id: stream.identity.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })?;

        let stream_grpc = grpc_uri(&stream)?;
        let mut stream_client = StreamControlClient::connect(stream_grpc)
            .await
            .map_err(|error| GuardError::Conflict(format!("connect stream RPC failed: {error}")))?;
        let stream_response = stream_client
            .start_receive(StartReceiveRequest {
                operation: Some(operation.clone()),
                stream_id: stream_id.clone(),
                route_id: route_id.clone(),
                lease_id: lease_id.clone(),
                expected_stream: Some(proto_identity(&stream.identity)),
                preferred_endpoints: receive_endpoints(&stream),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("start stream receive failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(stream_response.error) {
            let _ =
                LeaseService::new(self.store.clone()).fail(&lease_id, &stream.identity.instance_id);
            return Err(GuardError::Conflict(format!(
                "stream receive rejected: {} {}",
                error.code, error.message
            )));
        }
        if stream_response.state != StreamState::Receiving as i32 {
            let _ =
                LeaseService::new(self.store.clone()).fail(&lease_id, &stream.identity.instance_id);
            return Err(GuardError::Conflict(
                "stream did not enter receiving state".to_string(),
            ));
        }
        LeaseService::new(self.store.clone()).confirm(&lease_id, &stream.identity.instance_id)?;
        RouteService::new(self.store.clone()).apply_snapshot(ResourceSnapshot {
            owner: stream.identity.clone(),
            generation: 1,
            sequence: 1,
            resources: vec![SnapshotResource {
                resource_id: stream_id.clone(),
                route_id: Some(route_id.clone()),
            }],
        })?;

        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::connect(session_grpc)
                .await
                .map_err(|error| {
                    GuardError::Conflict(format!("connect session RPC failed: {error}"))
                })?;
        let request = StartDeviceStreamRequest {
            operation: Some(operation),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            route_id: route_id.clone(),
            lease_id: lease_id.clone(),
            expected_session: Some(proto_identity(&session.identity)),
        };
        let session_response = match kind {
            DeviceStreamKind::Live => session_client.start_live(request).await,
            DeviceStreamKind::Playback => session_client.start_playback(request).await,
            DeviceStreamKind::Download => session_client.start_download(request).await,
            DeviceStreamKind::Talk => session_client.start_talk(request).await,
        }
        .map_err(|error| {
            GuardError::Conflict(format!("start session {} failed: {error}", kind.prefix()))
        })?
        .into_inner();
        if let Some(error) = non_empty_error(session_response.error) {
            let _ = self
                .stop_stream_rpc(&stream, &stream_id, "session rejected")
                .await;
            let _ = LeaseService::new(self.store.clone())
                .release(&lease_id, &stream.identity.instance_id);
            return Err(GuardError::Conflict(format!(
                "session {} rejected: {} {}",
                kind.prefix(),
                error.code,
                error.message
            )));
        }
        if session_response.state != DeviceStreamState::Running as i32 {
            return Err(GuardError::Conflict(format!(
                "session did not enter {} running state",
                kind.prefix()
            )));
        }
        Ok(SimStream {
            stream_id: session_response.stream_id,
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            node_id: stream.identity.node_id,
            instance_id: stream.identity.instance_id,
            lease_id,
            route_id,
            endpoint: endpoint_label(stream_response.receive_endpoints),
            state: SimStreamState::Running,
        })
    }

    pub async fn stop_stream(&self, stream_id: &str) -> GuardResult<SimStream> {
        let route = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == stream_id && route.state != RouteState::Closed)
            .ok_or_else(|| GuardError::NotFound(format!("stream {stream_id}")))?;
        let stream = self
            .store
            .get_node(&route.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", route.node_id)))?;
        self.stop_stream_rpc(&stream, stream_id, "manual").await?;
        if let Some(mut stored_route) = self.store.get_route(&route.route_id) {
            stored_route.state = RouteState::Closed;
            self.store.upsert_route(stored_route);
        }
        if let Some(lease) =
            self.store.leases().into_iter().find(|lease| {
                lease.resource_id == stream_id && lease.state == LeaseState::Confirmed
            })
        {
            let _ = LeaseService::new(self.store.clone())
                .release(&lease.lease_id, &stream.identity.instance_id);
        }
        Ok(SimStream {
            stream_id: stream_id.to_string(),
            device_id: String::new(),
            channel_id: String::new(),
            node_id: stream.identity.node_id,
            instance_id: stream.identity.instance_id,
            lease_id: String::new(),
            route_id: route.route_id,
            endpoint: String::new(),
            state: SimStreamState::Stopped,
        })
    }

    pub async fn ptz(&self, device_id: &str, channel_id: &str) -> GuardResult<u64> {
        let session = self.select_node(NodeKind::Session, "device.ptz")?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::connect(session_grpc)
                .await
                .map_err(|error| {
                    GuardError::Conflict(format!("connect session RPC failed: {error}"))
                })?;
        let response = session_client
            .control_ptz(ControlPtzRequest {
                operation: Some(OperationRef {
                    operation_id: format!("ptz-{}", now_ms()),
                    idempotency_key: String::new(),
                }),
                device_id: device_id.to_string(),
                channel_id: channel_id.to_string(),
                command: "default".to_string(),
                speed: 1,
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("ptz RPC failed: {error}")))?
            .into_inner();
        if !response.accepted {
            let message = response
                .error
                .map(|error| format!("{} {}", error.code, error.message))
                .unwrap_or_else(|| "ptz rejected".to_string());
            return Err(GuardError::Conflict(message));
        }
        Ok(1)
    }

    pub async fn start_ai(
        &self,
        operation_id: &str,
        stream_id: &str,
        model: &str,
    ) -> GuardResult<SimAiTask> {
        let capability = ai_capability(model);
        let avai = self.select_node(NodeKind::Avai, &capability)?;
        let task_id = format!("ai-{operation_id}");
        let lease_id = format!("lease-ai-{operation_id}");
        let route_id = format!("route-ai-{operation_id}");
        let allocation =
            AllocationService::new(self.store.clone()).allocate(AllocationRequest {
                request_id: operation_id.to_string(),
                capability: capability.clone(),
                zone: avai.zone.clone(),
            })?;
        if allocation.owner.node_id != avai.identity.node_id {
            return Err(GuardError::Conflict(
                "selected avai node changed during allocation".to_string(),
            ));
        }
        LeaseService::new(self.store.clone()).allocate(LeaseRequest {
            lease_id: lease_id.clone(),
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            idempotency_key: format!("ai-{operation_id}"),
            owner: avai.identity.clone(),
            now_ms: now_ms(),
            ttl_ms: 30_000,
        })?;
        RouteService::new(self.store.clone()).create_allocated(RouteRecord {
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            node_id: avai.identity.node_id.clone(),
            instance_id: avai.identity.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })?;

        let avai_grpc = grpc_uri(&avai)?;
        let mut avai_client = AvaiControlClient::connect(avai_grpc)
            .await
            .map_err(|error| GuardError::Conflict(format!("connect avai RPC failed: {error}")))?;
        let response = avai_client
            .create_task(CreateTaskRequest {
                operation: Some(OperationRef {
                    operation_id: operation_id.to_string(),
                    idempotency_key: operation_id.to_string(),
                }),
                task_id: task_id.clone(),
                task_type: capability.clone(),
                route_id: route_id.clone(),
                expected_avai: Some(proto_identity(&avai.identity)),
                payload: format!(
                    "frame_ref={operation_id};stream_id={stream_id};expires_at_epoch_ms={}",
                    now_ms() + 30_000
                )
                .into_bytes(),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("create avai task failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            let _ =
                LeaseService::new(self.store.clone()).fail(&lease_id, &avai.identity.instance_id);
            return Err(GuardError::Conflict(format!(
                "avai task rejected: {} {}",
                error.code, error.message
            )));
        }
        if response.state != AiTaskState::Running as i32 {
            return Err(GuardError::Conflict(
                "avai task did not enter running state".to_string(),
            ));
        }
        LeaseService::new(self.store.clone()).confirm(&lease_id, &avai.identity.instance_id)?;
        RouteService::new(self.store.clone()).apply_snapshot(ResourceSnapshot {
            owner: avai.identity.clone(),
            generation: 1,
            sequence: 1,
            resources: vec![SnapshotResource {
                resource_id: task_id.clone(),
                route_id: Some(route_id.clone()),
            }],
        })?;
        Ok(SimAiTask {
            task_id: response.task_id,
            model: model.to_string(),
            stream_id: stream_id.to_string(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id,
            route_id,
            state: SimAiTaskState::Running,
        })
    }

    pub async fn cancel_ai(&self, task_id: &str) -> GuardResult<SimAiTask> {
        let route = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == task_id && route.state != RouteState::Closed)
            .ok_or_else(|| GuardError::NotFound(format!("AI task {task_id}")))?;
        let avai = self
            .store
            .get_node(&route.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", route.node_id)))?;
        let avai_grpc = grpc_uri(&avai)?;
        let mut avai_client = AvaiControlClient::connect(avai_grpc)
            .await
            .map_err(|error| GuardError::Conflict(format!("connect avai RPC failed: {error}")))?;
        let response = avai_client
            .cancel_task(CancelTaskRequest {
                operation: Some(OperationRef {
                    operation_id: format!("cancel-{task_id}"),
                    idempotency_key: String::new(),
                }),
                task_id: task_id.to_string(),
                reason: "manual".to_string(),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("cancel avai task failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            return Err(GuardError::Conflict(format!(
                "avai cancel rejected: {} {}",
                error.code, error.message
            )));
        }
        if response.state != AiTaskState::Cancelled as i32 {
            return Err(GuardError::Conflict(
                "avai task did not enter cancelled state".to_string(),
            ));
        }
        if let Some(mut stored_route) = self.store.get_route(&route.route_id) {
            stored_route.state = RouteState::Closed;
            self.store.upsert_route(stored_route);
        }
        if let Some(lease) = self
            .store
            .leases()
            .into_iter()
            .find(|lease| lease.resource_id == task_id && lease.state == LeaseState::Confirmed)
        {
            let _ = LeaseService::new(self.store.clone())
                .release(&lease.lease_id, &avai.identity.instance_id);
        }
        Ok(SimAiTask {
            task_id: task_id.to_string(),
            model: String::new(),
            stream_id: String::new(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id: String::new(),
            route_id: route.route_id,
            state: SimAiTaskState::Cancelled,
        })
    }

    fn select_node(&self, kind: NodeKind, capability: &str) -> GuardResult<NodeRecord> {
        self.store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == kind
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
                    && node.capabilities.iter().any(|item| item == capability)
            })
            .min_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id))
            .ok_or_else(|| GuardError::NotFound(format!("no {:?} node for {capability}", kind)))
    }

    async fn stop_stream_rpc(
        &self,
        stream: &NodeRecord,
        stream_id: &str,
        reason: &str,
    ) -> GuardResult<()> {
        let stream_grpc = grpc_uri(stream)?;
        let mut stream_client = StreamControlClient::connect(stream_grpc)
            .await
            .map_err(|error| GuardError::Conflict(format!("connect stream RPC failed: {error}")))?;
        let response = stream_client
            .stop_receive(StopReceiveRequest {
                operation: Some(OperationRef {
                    operation_id: format!("stop-{stream_id}"),
                    idempotency_key: String::new(),
                }),
                stream_id: stream_id.to_string(),
                reason: reason.to_string(),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("stop stream receive failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            return Err(GuardError::Conflict(format!(
                "stream stop rejected: {} {}",
                error.code, error.message
            )));
        }
        Ok(())
    }
}

fn grpc_uri(node: &NodeRecord) -> GuardResult<String> {
    let endpoint = node
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| {
            GuardError::NotFound(format!("node {} grpc endpoint", node.identity.node_id))
        })?;
    let scheme = if endpoint.scheme == "grpcs" {
        "https"
    } else {
        "http"
    };
    Ok(format!("{scheme}://{}:{}", endpoint.host, endpoint.port))
}

fn receive_endpoints(node: &NodeRecord) -> Vec<Endpoint> {
    node.endpoints
        .iter()
        .filter(|endpoint| endpoint.scheme != "grpc")
        .cloned()
        .map(proto_endpoint)
        .collect()
}

fn proto_identity(identity: &NodeIdentity) -> ProtoIdentity {
    ProtoIdentity {
        node_id: identity.node_id.clone(),
        instance_id: identity.instance_id.clone(),
        kind: match identity.kind {
            NodeKind::Session => ProtoNodeKind::Session,
            NodeKind::Stream => ProtoNodeKind::Stream,
            NodeKind::Avai => ProtoNodeKind::Avai,
        } as i32,
    }
}

fn proto_endpoint(endpoint: EndpointRecord) -> Endpoint {
    Endpoint {
        name: endpoint.name,
        scheme: endpoint.scheme,
        host: endpoint.host,
        port: endpoint.port,
        mode: match endpoint.mode {
            EndpointModeRecord::Single => EndpointMode::Single,
            EndpointModeRecord::Multi => EndpointMode::Multi,
        } as i32,
        labels: endpoint.labels,
    }
}

fn endpoint_label(endpoints: Vec<Endpoint>) -> String {
    endpoints
        .into_iter()
        .next()
        .map(|endpoint| format!("{}://{}:{}", endpoint.scheme, endpoint.host, endpoint.port))
        .unwrap_or_default()
}

fn non_empty_error(error: Option<ErrorDetail>) -> Option<ErrorDetail> {
    error.filter(|error| !error.code.is_empty() || !error.message.is_empty())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[derive(Debug, Clone, Copy)]
enum DeviceStreamKind {
    Live,
    Playback,
    Download,
    Talk,
}

impl DeviceStreamKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Playback => "playback",
            Self::Download => "download",
            Self::Talk => "talk",
        }
    }

    fn stream_capability(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Playback => "playback",
            Self::Download => "download",
            Self::Talk => "talk",
        }
    }

    fn session_capability(self) -> &'static str {
        match self {
            Self::Live => "device.live",
            Self::Playback => "device.playback",
            Self::Download => "device.download",
            Self::Talk => "device.talk",
        }
    }
}

fn ai_capability(model: &str) -> String {
    if model.starts_with("ai.") {
        model.to_string()
    } else {
        format!("ai.{model}")
    }
}
