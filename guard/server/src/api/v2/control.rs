use std::time::Duration;

use gmv_protocol::avai::v1::avai_control_client::AvaiControlClient;
use gmv_protocol::avai::v1::{AiTaskState, CancelTaskRequest, CreateTaskRequest};
use gmv_protocol::common::v1::{
    ErrorDetail, NodeIdentity as ProtoIdentity, NodeKind as ProtoNodeKind, OperationRef,
};
use gmv_protocol::session::v1::session_control_client::SessionControlClient;
use gmv_protocol::session::v1::{
    ControlPtzRequest, DeviceStreamState, StartDeviceStreamRequest, StopDeviceStreamRequest,
};

use crate::api::v2::model::{AiTaskSummary, AiTaskSummaryState, StreamSummary, StreamSummaryState};
use crate::core::{
    ConnectionState, GuardError, GuardResult, LeaseState, NodeIdentity, NodeKind, RouteState,
    SchedulingState,
};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::store::InMemoryGuardStore;
use crate::store::model::{NodeRecord, RouteRecord};

#[derive(Debug, Clone)]
pub struct BusinessControl {
    store: InMemoryGuardStore,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceStreamOptions {
    pub token: String,
    pub start_time_sec: u32,
    pub end_time_sec: u32,
    pub trans_mode: String,
    pub output_type: String,
    pub talk_codec: String,
    pub talk_sample_rate: u32,
    pub talk_channel_count: u32,
    pub talk_frame_duration_ms: u32,
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
    ) -> GuardResult<StreamSummary> {
        self.start_live_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_live_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Live,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_playback(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_playback_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_playback_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Playback,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_download(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_download_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_download_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Download,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_talk(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_talk_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_talk_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Talk,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    async fn start_device_stream(
        &self,
        kind: DeviceStreamKind,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        let session = self.select_node(NodeKind::Session, kind.session_capability())?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let operation = OperationRef {
            operation_id: operation_id.to_string(),
            idempotency_key: operation_id.to_string(),
        };
        let token = if options.token.trim().is_empty() {
            format!("gmv-{operation_id}")
        } else {
            options.token
        };
        let request = StartDeviceStreamRequest {
            operation: Some(operation),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            route_id: String::new(),
            lease_id: String::new(),
            expected_session: Some(proto_identity(&session.identity)),
            token,
            start_time_sec: options.start_time_sec,
            end_time_sec: options.end_time_sec,
            trans_mode: options.trans_mode,
            output_type: options.output_type,
            talk_codec: options.talk_codec,
            talk_sample_rate: options.talk_sample_rate,
            talk_channel_count: options.talk_channel_count,
            talk_frame_duration_ms: options.talk_frame_duration_ms,
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
        let lease = self
            .store
            .leases()
            .into_iter()
            .find(|lease| lease.resource_id == session_response.stream_id);
        let route = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == session_response.stream_id);
        Ok(StreamSummary {
            stream_id: session_response.stream_id,
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            node_id: route
                .as_ref()
                .map(|route| route.node_id.clone())
                .unwrap_or_else(|| session.identity.node_id.clone()),
            instance_id: route
                .as_ref()
                .map(|route| route.instance_id.clone())
                .unwrap_or_else(|| session.identity.instance_id.clone()),
            lease_id: lease.map(|lease| lease.lease_id).unwrap_or_default(),
            route_id: route.map(|route| route.route_id).unwrap_or_default(),
            endpoint: session_response.endpoint,
            state: StreamSummaryState::Running,
        })
    }

    pub async fn stop_stream(&self, stream_id: &str) -> GuardResult<StreamSummary> {
        let session = self.select_any_session()?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let response = session_client
            .stop_device_stream(StopDeviceStreamRequest {
                operation: Some(OperationRef {
                    operation_id: format!("stop-{stream_id}"),
                    idempotency_key: String::new(),
                }),
                stream_id: stream_id.to_string(),
                reason: "manual".to_string(),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("stop session stream failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            return Err(GuardError::Conflict(format!(
                "session stop rejected: {} {}",
                error.code, error.message
            )));
        }
        if let Some(route) = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == stream_id && route.state != RouteState::Closed)
        {
            if let Some(mut stored_route) = self.store.get_route(&route.route_id) {
                stored_route.state = RouteState::Closed;
                self.store.upsert_route(stored_route);
            }
        }
        Ok(StreamSummary {
            stream_id: stream_id.to_string(),
            device_id: String::new(),
            channel_id: String::new(),
            node_id: session.identity.node_id,
            instance_id: session.identity.instance_id,
            lease_id: String::new(),
            route_id: String::new(),
            endpoint: String::new(),
            state: StreamSummaryState::Stopped,
        })
    }

    pub async fn ptz(&self, device_id: &str, channel_id: &str) -> GuardResult<u64> {
        let session = self.select_node(NodeKind::Session, "device.ptz")?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
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
    ) -> GuardResult<AiTaskSummary> {
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
        let mut avai_client = AvaiControlClient::new(connect_rpc(&avai_grpc, "avai").await?);
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
        Ok(AiTaskSummary {
            task_id: response.task_id,
            model: model.to_string(),
            stream_id: stream_id.to_string(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id,
            route_id,
            state: AiTaskSummaryState::Running,
        })
    }

    pub async fn cancel_ai(&self, task_id: &str) -> GuardResult<AiTaskSummary> {
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
        let mut avai_client = AvaiControlClient::new(connect_rpc(&avai_grpc, "avai").await?);
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
        Ok(AiTaskSummary {
            task_id: task_id.to_string(),
            model: String::new(),
            stream_id: String::new(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id: String::new(),
            route_id: route.route_id,
            state: AiTaskSummaryState::Cancelled,
        })
    }

    fn select_any_session(&self) -> GuardResult<NodeRecord> {
        self.store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == NodeKind::Session
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
            })
            .min_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id))
            .ok_or_else(|| GuardError::NotFound("no connected session node".to_string()))
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
}

async fn connect_rpc(uri: &str, name: &str) -> GuardResult<tonic::transport::Channel> {
    let mut config = base_rpc::RpcChannelConfig::new(uri.to_string());
    if uri.starts_with("https://") {
        config.tls = Some(base_rpc::RpcClientTlsConfig {
            domain_name: url::Url::parse(uri)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string)),
            ca_certificate_pem: None,
            client_certificate_pem: None,
            client_private_key_pem: None,
            use_native_roots: true,
            handshake_timeout: Duration::from_secs(5),
        });
    }
    base_rpc::connect_channel(&config)
        .await
        .map_err(|error| GuardError::Conflict(format!("connect {name} RPC failed: {error}")))
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
