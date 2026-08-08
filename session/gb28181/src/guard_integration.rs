use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use base::chrono::{Duration as TimeDelta, Local, TimeZone};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error as log_error, info, warn};
use base::serde::de::DeserializeOwned;
use base_rpc::RpcChannelConfig;
use gmv_domain::info::format::{CMaf, Flv, Mp4};
use gmv_domain::info::media_info::{OutputAudioCodec, TranscodeConfig};
use gmv_domain::info::obj::{
    BroadcastClosedEvent, BroadcastStartModel, BroadcastStopModel, OutputStreamInfo,
    RegisterStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState, UnknownStreamEvent,
};
use gmv_domain::info::output::{
    DashFmp4Output, HlsFmp4Output, HlsPlaylistProfile, HttpFlvOutput, LocalMp4Output, OutputKind,
};
use gmv_nodec::NodeEventSender;
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity, NodeKind, OperationRef, ResourceRef,
};
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, AllocateStreamResponse, EventPriority, LeaseRequest, NodeEvent,
    NodeHealth, NodeHeartbeat, NodeResourceSnapshot, NodeToGuardMessage, QueryNodeRequest,
    RegisterNodeRequest, ResourceReport, ResourceState, guard_control_client::GuardControlClient,
    node_to_guard_message,
};
use gmv_protocol::session::v1::{
    ActiveStreamDialogItem, ActiveStreamItem, ActiveStreamManagementState,
    ActiveStreamViewerFormat, CloudRecordingFileState, CloudRecordingResponse,
    CloudRecordingStatus, CloudRecordingSummary, ControlPtzRequest, ControlPtzResponse,
    CreateCloudRecordingRequest, CreateGbDeviceRequest, CreateGbDeviceResponse,
    DeleteCloudRecordingRequest, DeleteGbDeviceRequest, DeleteGbDeviceResponse,
    DeviceStreamResponse, DeviceStreamState, GbChannel, GbChannelImage, GbDevice,
    GbRecordQueryBatch, GbRecordSegment, GbResource, GbResourceConfirmation, GbResourceResponse,
    GetActiveStreamManagementRequest, GetActiveStreamManagementResponse, GetCloudRecordingRequest,
    GetGbChannelRecordsRequest, GetGbChannelRecordsResponse, GetGbChannelRequest,
    GetGbChannelResponse, GetGbDeviceRequest, GetGbDeviceResponse, GetSessionConfigRequest,
    GetSessionConfigResponse, IssueCloudRecordingAccessRequest, IssueCloudRecordingAccessResponse,
    IssueGbChannelImageAccessRequest, IssueGbChannelImageAccessResponse,
    ListActiveStreamDialogsRequest, ListActiveStreamDialogsResponse, ListActiveStreamsRequest,
    ListActiveStreamsResponse, ListCloudRecordingsRequest, ListCloudRecordingsResponse,
    ListGbChannelImagesRequest, ListGbChannelImagesResponse, ListGbChannelsRequest,
    ListGbChannelsResponse, ListGbDevicesRequest, ListGbDevicesResponse, ListGbResourcesRequest,
    ListGbResourcesResponse, ListStreamHistoryRequest, ListStreamHistoryResponse,
    PlaybackControlResponse, PlaybackPresenceHeartbeatResult, PlaybackState,
    QueryGbChannelRecordsRequest, RefreshPlaybackPresenceRequest, RefreshPlaybackPresenceResponse,
    ResetGbResourceConfirmationRequest, SaveGbResourceConfirmationRequest, SeekPlaybackRequest,
    SessionHookRequest, SessionHookResponse, SetGbChannelCoverRequest, SetPlaybackSpeedRequest,
    SetPlaybackSpeedResponse, SetPlaybackStateRequest, SnapshotImageRequest, SnapshotImageResponse,
    StartDeviceStreamRequest, StopCloudRecordingRequest, StopDeviceStreamRequest,
    StreamHistoryItem, StreamProfileVerification, UpdateGbChannelRequest, UpdateGbChannelResponse,
    UpdateGbDeviceRequest, UpdateGbDeviceResponse, VideoStreamProfile,
    session_control_server::SessionControl, session_hook_server::SessionHook,
};
use gmv_protocol::stream::v1::{
    StartReceiveRequest, StartReceiveResponse, StreamState as ProtoStreamState,
};
use tonic::transport::Channel;

use crate::service::{
    api_serv, dialog_recovery, edge_serv, hook_serv, playback_presence, record_query, stream_close,
    stream_rpc,
};
use crate::state::model::{
    DeviceChannelIdent, LiveStreamProfile, PlayBackModel, PlayLiveModel, PlaySeekModel,
    PlaySpeedModel, PtzControlModel, SnapshotImage, TransMode,
};
use crate::state::session::GuardLease;
use crate::state::{StreamNode, StreamNodeRegistry};
use crate::storage::dialog_session::{
    DialogMonitorFilter, DialogSessionType, DialogState, SipDialogSession,
    SipDialogSessionRepository,
};

static GUARD_EVENT_SENDER: OnceLock<NodeEventSender> = OnceLock::new();
static GUARD_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn rpc_channel_config(endpoint: String) -> RpcChannelConfig {
    let mut config = RpcChannelConfig::new(endpoint.clone());
    if endpoint.starts_with("https://") {
        config.tls = Some(base_rpc::RpcClientTlsConfig {
            domain_name: url::Url::parse(&endpoint)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string)),
            ca_certificate_pem: None,
            client_certificate_pem: None,
            client_private_key_pem: None,
            use_native_roots: true,
            handshake_timeout: std::time::Duration::from_secs(5),
        });
    }
    config
}

pub fn init_guard_event_sender(sender: NodeEventSender) {
    let _ = GUARD_EVENT_SENDER.set(sender);
}

async fn guard_control_client() -> GlobalResult<GuardControlClient<Channel>> {
    let endpoint = crate::state::GuardConf::get_or_default().endpoint;
    let started = Instant::now();
    base::log::debug!("session rpc client outbound: service=guard_control, endpoint={endpoint}");
    let channel = base_rpc::connect_channel(&rpc_channel_config(endpoint.clone()))
        .await
        .map_err(|err| {
            base::log::debug!(
                "session rpc client inbound: service=guard_control, endpoint={endpoint}, status=error, elapsed_ms={}, err={err:?}",
                started.elapsed().as_millis()
            );
            GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "connect guard control rpc failed",
                |msg| log_error!("{msg}: endpoint={endpoint}, err={err:?}"),
            )
        })?;
    base::log::debug!(
        "session rpc client inbound: service=guard_control, endpoint={endpoint}, status=ok, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(GuardControlClient::new(channel))
}

#[derive(Debug, Clone)]
pub struct AllocatedStreamNode {
    pub node: StreamNode,
    pub media_endpoint: Endpoint,
    pub lease_id: String,
    pub route_id: String,
    pub instance_id: String,
}

pub async fn allocate_stream_node(
    operation_id: &str,
    stream_id: &str,
    stream_type: &str,
    device_id: &str,
    channel_id: &str,
) -> GlobalResult<AllocatedStreamNode> {
    allocate_stream_node_with_constraints(
        operation_id,
        stream_id,
        stream_type,
        device_id,
        channel_id,
        HashMap::new(),
    )
    .await
}

pub async fn allocate_stream_node_with_constraints(
    operation_id: &str,
    stream_id: &str,
    stream_type: &str,
    device_id: &str,
    channel_id: &str,
    extra_constraints: HashMap<String, String>,
) -> GlobalResult<AllocatedStreamNode> {
    let mut client = guard_control_client().await?;
    let mut constraints = HashMap::from([
        ("device_id".to_string(), device_id.to_string()),
        ("channel_id".to_string(), channel_id.to_string()),
    ]);
    constraints.extend(extra_constraints);
    let response = client
        .allocate_stream(AllocateStreamRequest {
            operation: Some(operation(operation_id)),
            stream_id: stream_id.to_string(),
            stream_type: stream_type.to_string(),
            constraints,
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    let node = stream_node_from_allocation(&response)?;
    let media_endpoint = response
        .endpoints
        .iter()
        .find(|endpoint| {
            (endpoint.name == "rtp" || endpoint.scheme == "rtp")
                && endpoint.mode == EndpointMode::Single as i32
        })
        .cloned()
        .ok_or_else(|| missing_stream_endpoint(&node.name, "rtp"))?;
    Ok(AllocatedStreamNode {
        node,
        media_endpoint,
        lease_id: response.lease_id,
        route_id: response.route_id,
        instance_id: response
            .stream_node
            .map(|identity| identity.instance_id)
            .unwrap_or_default(),
    })
}

pub async fn ensure_stream_node(node_id: &str) -> GlobalResult<StreamNode> {
    if let Some(node) = StreamNodeRegistry::get(node_id) {
        return Ok(node);
    }
    let mut client = guard_control_client().await?;
    let response = client
        .query_node(QueryNodeRequest {
            node_id: node_id.to_string(),
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    let identity = response.current.ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "guard query node response has no identity",
            |msg| log_error!("{msg}: node={node_id}"),
        )
    })?;
    let node = stream_node_from_parts(&identity.node_id, response.endpoints, false)?;
    StreamNodeRegistry::upsert(node.clone());
    Ok(node)
}

impl AllocatedStreamNode {
    pub fn guard_lease(&self) -> GuardLease {
        GuardLease {
            lease_id: self.lease_id.clone(),
            route_id: self.route_id.clone(),
            instance_id: self.instance_id.clone(),
        }
    }
}

pub async fn confirm_stream_lease(allocation: &AllocatedStreamNode) -> GlobalResult<()> {
    let mut client = guard_control_client().await?;
    let _ = client
        .confirm_lease(LeaseRequest {
            lease_id: allocation.lease_id.clone(),
            route_id: allocation.route_id.clone(),
            expected_instance_id: allocation.instance_id.clone(),
            error: None,
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?;
    Ok(())
}

pub async fn fail_stream_lease(allocation: &AllocatedStreamNode, reason: &str) {
    let Ok(mut client) = guard_control_client().await else {
        warn!(
            "skip guard lease fail: lease_id={}, reason=guard_unavailable",
            allocation.lease_id
        );
        return;
    };
    let _ = client
        .fail_lease(LeaseRequest {
            lease_id: allocation.lease_id.clone(),
            route_id: allocation.route_id.clone(),
            expected_instance_id: allocation.instance_id.clone(),
            error: Some(error("stream_start_failed", reason)),
        })
        .await
        .map_err(|err| {
            warn!(
                "guard lease fail rejected: lease_id={}, err={err:?}",
                allocation.lease_id
            )
        });
}

pub async fn release_stream_lease(lease: GuardLease) {
    if lease.lease_id.is_empty() || lease.instance_id.is_empty() {
        return;
    }
    let Ok(mut client) = guard_control_client().await else {
        warn!(
            "skip guard lease release: lease_id={}, reason=guard_unavailable",
            lease.lease_id
        );
        return;
    };
    let _ = client
        .release_lease(LeaseRequest {
            lease_id: lease.lease_id.clone(),
            route_id: lease.route_id.clone(),
            expected_instance_id: lease.instance_id.clone(),
            error: None,
        })
        .await
        .map_err(|err| {
            warn!(
                "guard lease release rejected: lease_id={}, err={err:?}",
                lease.lease_id
            )
        });
}

async fn release_subscription_outputs(
    stream_id: &str,
    subscription_id: &str,
    operation_id: &str,
) -> GlobalResult<Vec<String>> {
    let Some((node_id, _)) =
        crate::state::session::Cache::stream_map_query_node(&stream_id.to_string())
    else {
        return Ok(Vec::new());
    };
    let node = match StreamNodeRegistry::get(&node_id) {
        Some(node) => node,
        None => ensure_stream_node(&node_id).await?,
    };
    stream_rpc::release_subscription_outputs(&node, operation_id, stream_id, subscription_id).await
}

fn stream_node_from_allocation(allocation: &AllocateStreamResponse) -> GlobalResult<StreamNode> {
    let identity = allocation.stream_node.as_ref().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "guard allocation response has no stream node",
            |msg| log_error!("{msg}: lease_id={}", allocation.lease_id),
        )
    })?;
    stream_node_from_parts(&identity.node_id, allocation.endpoints.clone(), true)
}

fn stream_node_from_parts(
    node_id: &str,
    endpoints: Vec<Endpoint>,
    require_concrete_rtp: bool,
) -> GlobalResult<StreamNode> {
    let grpc = endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| missing_stream_endpoint(node_id, "grpc"))?;
    let rtp = endpoints
        .iter()
        .find(|endpoint| {
            (endpoint.name == "rtp" || endpoint.scheme == "rtp")
                && (!require_concrete_rtp || endpoint.mode == EndpointMode::Single as i32)
        })
        .ok_or_else(|| missing_stream_endpoint(node_id, "rtp"))?;
    Ok(StreamNode {
        name: node_id.to_string(),
        control_grpc_uri: base_rpc::rpc_endpoint_uri(
            grpc.scheme == "grpcs",
            &grpc.host,
            u16::try_from(grpc.port).unwrap_or(u16::MAX),
        ),
        pub_host: rtp.host.clone(),
        pub_port: u16::try_from(rtp.port).unwrap_or(u16::MAX),
    })
}

fn missing_stream_endpoint(node_id: &str, endpoint: &str) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::NotFound.code(),
        "stream node endpoint is missing",
        |msg| log_error!("{msg}: node={node_id}, endpoint={endpoint}"),
    )
}

pub async fn guard_record_running(device_id: &str, channel_id: &str) -> GlobalResult<bool> {
    crate::storage::recording::running_record_exists(device_id, channel_id).await
}

pub async fn guard_record_started(
    biz_id: &str,
    device_id: &str,
    channel_id: &str,
    st_epoch_sec: i64,
    et_epoch_sec: i64,
    speed: u32,
    stream_app_name: &str,
) -> GlobalResult<()> {
    crate::storage::recording::start_record(crate::storage::recording::RecordStart {
        biz_id,
        device_id,
        channel_id,
        st_epoch_sec,
        et_epoch_sec,
        speed,
        stream_app_name,
    })
    .await
}

pub async fn guard_record_finished(
    biz_id: &str,
    reported_state: u8,
    file_size: u64,
    record_duration_sec: u64,
    file_format: &str,
    dir_path: &str,
    abs_path: &str,
) -> GlobalResult<()> {
    let finished =
        crate::storage::recording::finish_record(crate::storage::recording::RecordFinish {
            biz_id,
            reported_state,
            file_size,
            record_duration_sec,
            file_format,
            dir_path,
            abs_path,
        })
        .await?;
    if finished {
        Ok(())
    } else {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "record not found",
            |msg| log_error!("{msg}: biz_id={biz_id}"),
        ))
    }
}

pub fn publish_guard_event(topic: &str, payload: impl Into<Vec<u8>>) {
    let payload = payload.into();
    let Some(sender) = GUARD_EVENT_SENDER.get() else {
        base::log::warn!(
            "guard event outbound skipped: topic={topic}, reason=event_sender_not_initialized, payload_bytes={}",
            payload.len()
        );
        return;
    };
    let sequence = GUARD_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let event_id = format!("session-event-{sequence}");
    base::log::info!(
        "guard event outbound: event_id={event_id}, topic={topic}, payload_bytes={}",
        payload.len()
    );
    let event = NodeEvent {
        event_id,
        topic: topic.to_string(),
        priority: EventPriority::P1 as i32,
        payload,
    };
    if let Err(error) = sender.try_send(event) {
        base::log::warn!("drop guard session event {topic}: {error}");
    }
}

#[derive(Debug, Clone)]
pub struct SessionGuardNode {
    pub guard_channel: RpcChannelConfig,
    pub identity: NodeIdentity,
    pub software_version: String,
    pub started_at_epoch_ms: i64,
    pub endpoints: Vec<Endpoint>,
    pub capabilities: Vec<String>,
}

impl SessionGuardNode {
    pub fn new(
        node_id: impl Into<String>,
        instance_id: impl Into<String>,
        http_endpoint: (bool, String, u16),
    ) -> Self {
        let (tls, host, port) = http_endpoint;
        let endpoints = vec![Endpoint {
            name: "http".to_string(),
            scheme: if tls { "https" } else { "http" }.to_string(),
            host,
            port: u32::from(port),
            mode: EndpointMode::Single as i32,
            labels: HashMap::new(),
        }];
        Self {
            guard_channel: rpc_channel_config(crate::state::GuardConf::get_or_default().endpoint),
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Session as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            endpoints,
            capabilities: vec![
                "device.live".to_string(),
                "device.playback".to_string(),
                "device.download".to_string(),
                "device.cloud_recording".to_string(),
                "device.broadcast".to_string(),
                "device.ptz".to_string(),
                "protocol.gb28181".to_string(),
            ],
        }
    }

    pub fn register_request(&self, snapshot: NodeResourceSnapshot) -> RegisterNodeRequest {
        RegisterNodeRequest {
            identity: Some(self.identity.clone()),
            software_version: self.software_version.clone(),
            started_at_epoch_ms: self.started_at_epoch_ms,
            endpoints: self.endpoints.clone(),
            capabilities: self.capabilities.clone(),
            startup_snapshot: Some(snapshot),
            host_metrics: None,
            zone: String::new(),
            takeover: false,
            config: self.config_summary(),
        }
    }

    fn config_summary(&self) -> HashMap<String, String> {
        HashMap::from([
            ("node_id".to_string(), self.identity.node_id.clone()),
            ("domain_id".to_string(), self.identity.node_id.clone()),
            ("service".to_string(), "session-gb28181".to_string()),
            ("protocol".to_string(), "gb28181".to_string()),
            (
                "display_name".to_string(),
                format!("GB28181 会话节点 {}", self.identity.node_id),
            ),
            (
                "software_version".to_string(),
                self.software_version.clone(),
            ),
            (
                "endpoint_count".to_string(),
                self.endpoints.len().to_string(),
            ),
        ])
    }

    pub fn heartbeat_message(
        &self,
        sequence: u64,
        sent_at_epoch_ms: i64,
        active_dialogs: usize,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence,
            sent_at_epoch_ms,
            payload: Some(node_to_guard_message::Payload::Heartbeat(NodeHeartbeat {
                health: NodeHealth::Ready as i32,
                host_metrics: None,
                metrics: HashMap::from([(
                    "active_dialogs".to_string(),
                    active_dialogs.to_string(),
                )]),
            })),
        }
    }

    pub fn snapshot_message(
        &self,
        sequence: u64,
        sent_at_epoch_ms: i64,
        snapshot: NodeResourceSnapshot,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence,
            sent_at_epoch_ms,
            payload: Some(node_to_guard_message::Payload::Snapshot(snapshot)),
        }
    }
}

#[derive(Clone)]
pub struct SessionControlRpc {
    inner: Arc<Mutex<SessionControlAdapter>>,
}

impl SessionControlRpc {
    pub fn new(adapter: SessionControlAdapter) -> Self {
        Self {
            inner: Arc::new(Mutex::new(adapter)),
        }
    }
}

#[tonic::async_trait]
impl SessionControl for SessionControlRpc {
    async fn create_cloud_recording(
        &self,
        request: tonic::Request<CreateCloudRecordingRequest>,
    ) -> Result<tonic::Response<CloudRecordingResponse>, tonic::Status> {
        let request = request.into_inner();
        let node_id = self.session_node_id()?;
        if request.session_node_id != node_id {
            return Err(tonic::Status::failed_precondition("stale_instance"));
        }
        let record =
            crate::service::cloud_recording::create(crate::service::cloud_recording::CreateInput {
                request_id: &request.request_id,
                session_node_id: &request.session_node_id,
                device_id: &request.device_id,
                channel_id: &request.channel_id,
                requested_by: &request.requested_by,
                start_time_sec: request.start_time_sec,
                end_time_sec: request.end_time_sec,
            })
            .await
            .map_err(storage_status)?;
        Ok(tonic::Response::new(CloudRecordingResponse {
            recording: Some(cloud_recording_proto(record)),
        }))
    }

    async fn list_cloud_recordings(
        &self,
        request: tonic::Request<ListCloudRecordingsRequest>,
    ) -> Result<tonic::Response<ListCloudRecordingsResponse>, tonic::Status> {
        let request = request.into_inner();
        let page = request.page.max(1);
        let page_size = request.page_size.clamp(1, 100);
        let (records, total) = crate::service::cloud_recording::list(
            &request.device_id,
            &request.channel_id,
            page,
            page_size,
            request.include_deleted,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(ListCloudRecordingsResponse {
            recordings: records.into_iter().map(cloud_recording_proto).collect(),
            total,
            page,
            page_size,
        }))
    }

    async fn get_cloud_recording(
        &self,
        request: tonic::Request<GetCloudRecordingRequest>,
    ) -> Result<tonic::Response<CloudRecordingResponse>, tonic::Status> {
        let record =
            crate::service::cloud_recording::get_with_progress(&request.into_inner().task_id)
                .await
                .map_err(storage_status)?;
        Ok(tonic::Response::new(CloudRecordingResponse {
            recording: Some(cloud_recording_proto(record)),
        }))
    }

    async fn stop_cloud_recording(
        &self,
        request: tonic::Request<StopCloudRecordingRequest>,
    ) -> Result<tonic::Response<CloudRecordingResponse>, tonic::Status> {
        let record = crate::service::cloud_recording::stop(&request.into_inner().task_id)
            .await
            .map_err(storage_status)?;
        Ok(tonic::Response::new(CloudRecordingResponse {
            recording: Some(cloud_recording_proto(record)),
        }))
    }

    async fn delete_cloud_recording(
        &self,
        request: tonic::Request<DeleteCloudRecordingRequest>,
    ) -> Result<tonic::Response<CloudRecordingResponse>, tonic::Status> {
        let record = crate::service::cloud_recording::delete(&request.into_inner().task_id)
            .await
            .map_err(storage_status)?;
        Ok(tonic::Response::new(CloudRecordingResponse {
            recording: Some(cloud_recording_proto(record)),
        }))
    }

    async fn issue_cloud_recording_access(
        &self,
        request: tonic::Request<IssueCloudRecordingAccessRequest>,
    ) -> Result<tonic::Response<IssueCloudRecordingAccessResponse>, tonic::Status> {
        let request = request.into_inner();
        let issued =
            crate::http::cloud_recording::issue_ticket(&request.task_id, &request.mode).await?;
        Ok(tonic::Response::new(IssueCloudRecordingAccessResponse {
            url: issued.url,
            expires_at_ms: issued.expires_at_ms,
            content_type: issued.content_type,
            file_name: issued.file_name,
            file_size: issued.file_size,
        }))
    }

    async fn start_live(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        self.start_device_stream(request, "live").await
    }

    async fn start_playback(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        self.start_device_stream(request, "playback").await
    }

    async fn start_download(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        self.start_device_stream(request, "download").await
    }

    async fn start_broadcast(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        self.start_device_stream(request, "broadcast").await
    }

    async fn stop_device_stream(
        &self,
        request: tonic::Request<StopDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        let mut request = request.into_inner();
        request.stop_reason = request.stop_reason.trim().to_string();
        if request.expected_session.is_some()
            && request.force
            && request.reason == "manual_stop"
            && (request.stop_reason.is_empty()
                || request.stop_reason.chars().count() > 255
                || request.stop_reason.contains('\0'))
        {
            return Err(tonic::Status::invalid_argument(
                "stop_reason must be in 1..=255 characters",
            ));
        }
        debug!(
            "session_control.stop_device_stream: stream_id={}, reason={}, force={}, has_stop_reason={}",
            request.stream_id,
            request.reason,
            request.force,
            !request.stop_reason.is_empty()
        );
        let identity = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))?
            .identity
            .clone();
        if request.expected_session.is_some()
            && (request.expected_session.as_ref().is_none_or(|expected| {
                expected.node_id != identity.node_id || expected.instance_id != identity.instance_id
            }))
        {
            return Err(tonic::Status::failed_precondition("stale_instance"));
        }
        let monitored_dialog = if request.expected_session.is_some() {
            let dialog = SipDialogSessionRepository::find_by_stream_id(&request.stream_id)
                .await
                .map_err(storage_status)?
                .ok_or_else(|| tonic::Status::not_found("stream dialog not found"))?;
            if dialog.signal_node_id != identity.node_id {
                return Err(tonic::Status::failed_precondition("wrong_session"));
            }
            if dialog.state == DialogState::Terminated {
                let mut response =
                    device_response(&request.stream_id, DeviceStreamState::Stopped, None);
                response.session_node_id = identity.node_id;
                response.session_instance_id = identity.instance_id;
                return Ok(tonic::Response::new(response));
            }
            if dialog.state == DialogState::Orphan {
                let err = GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    "stream closed abnormally",
                    |msg| {
                        warn!(
                            "{msg}: stream_id={}, terminal_reason={}, error_code={}",
                            request.stream_id,
                            dialog.terminal_reason.as_deref().unwrap_or("unknown"),
                            dialog.error_code.as_deref().unwrap_or("UNKNOWN")
                        )
                    },
                );
                let mut response = device_error(err);
                response.stream_id = request.stream_id.clone();
                response.session_node_id = identity.node_id;
                response.session_instance_id = identity.instance_id;
                return Ok(tonic::Response::new(response));
            }
            if request.force && request.reason == "manual_stop" {
                SipDialogSessionRepository::record_stop_reason(
                    &request.stream_id,
                    &identity.node_id,
                    &request.stop_reason,
                )
                .await
                .map_err(storage_status)?;
            }
            Some(dialog)
        } else {
            None
        };
        if let Some(dialog) = monitored_dialog.as_ref()
            && crate::state::session::Cache::broadcast_map_get(&request.stream_id).is_none()
            && crate::state::session::Cache::stream_map_query_input(&request.stream_id).is_none()
        {
            if dialog.state == DialogState::Inviting {
                if dialog.session_type != DialogSessionType::Broadcast {
                    let err = stream_close::close_unlinked_inviting_stream(dialog).await;
                    let mut response = device_error(err);
                    response.stream_id = request.stream_id.clone();
                    response.session_node_id = identity.node_id;
                    response.session_instance_id = identity.instance_id;
                    return Ok(tonic::Response::new(response));
                }
                let media_result = if dialog.session_type == DialogSessionType::Broadcast {
                    match ensure_stream_node(&dialog.media_node_id).await {
                        Ok(node) => stream_rpc::broadcast_close(
                            &node,
                            dialog
                                .parent_stream_id
                                .as_deref()
                                .unwrap_or(&dialog.stream_id),
                            &dialog.stream_id,
                        )
                        .await
                        .map(|_| ()),
                        Err(err) => Err(err),
                    }
                } else {
                    stream_close::stop_media_runtime(
                        &dialog.stream_id,
                        &dialog.media_node_id,
                        "manual_stop",
                    )
                    .await
                };
                let sip_result = crate::gb::sip::command::invite_stop_by_device(
                    &dialog.device_id,
                    crate::gb::sip::InviteStopRequest {
                        call_id: Some(dialog.call_id.clone()),
                        stream_id: Some(dialog.stream_id.clone()),
                        terminal_reason: "manual_stop".to_string(),
                    },
                )
                .await;
                let result = match (media_result, sip_result) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(media), Ok(())) => Err(media),
                    (Ok(()), Err(sip)) => Err(sip),
                    (Err(media), Err(sip)) => {
                        warn!(
                            "inviting stream manual stop failed: stream_id={}, media_error={}, sip_error={}",
                            dialog.stream_id, media, sip
                        );
                        Err(sip)
                    }
                };
                let mut response = match result {
                    Ok(()) => device_response(&request.stream_id, DeviceStreamState::Stopped, None),
                    Err(err) => device_error(err),
                };
                response.session_node_id = identity.node_id;
                response.session_instance_id = identity.instance_id;
                return Ok(tonic::Response::new(response));
            }
            if let Err(err) = dialog_recovery::recover_dialog(dialog).await {
                return Ok(tonic::Response::new(device_error(err)));
            }
            if let Some(current) = SipDialogSessionRepository::find_by_stream_id(&request.stream_id)
                .await
                .map_err(storage_status)?
            {
                if current.state == DialogState::Terminated {
                    let mut response =
                        device_response(&request.stream_id, DeviceStreamState::Stopped, None);
                    response.session_node_id = identity.node_id;
                    response.session_instance_id = identity.instance_id;
                    return Ok(tonic::Response::new(response));
                }
                if current.state == DialogState::Orphan {
                    let err = GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "stream recovery closed abnormally",
                        |msg| warn!("{msg}: stream_id={}", request.stream_id),
                    );
                    let mut response = device_error(err);
                    response.stream_id = request.stream_id.clone();
                    response.session_node_id = identity.node_id;
                    response.session_instance_id = identity.instance_id;
                    return Ok(tonic::Response::new(response));
                }
            }
        }
        let force = request.force || request.subscription_id.is_empty();
        let setup_lock = crate::state::session::Cache::stream_map_query_input(&request.stream_id)
            .map(|(device_id, channel_id, access_mode)| {
                crate::state::session::Cache::stream_setup_lock(
                    &device_id,
                    &channel_id,
                    access_mode,
                )
            });
        let _setup_guard = match setup_lock.as_ref() {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        if !force {
            if let Err(err) = release_subscription_outputs(
                &request.stream_id,
                &request.subscription_id,
                &operation_id(request.operation.as_ref()),
            )
            .await
            {
                return Ok(tonic::Response::new(device_response(
                    &request.stream_id,
                    DeviceStreamState::Running,
                    Some(gmv_nodec::error::global_error_detail(
                        "stream_output_release_failed",
                        &err,
                    )),
                )));
            }
            playback_presence::clear_for_subscription(&request.stream_id, &request.subscription_id);
        }
        let state = if crate::state::session::Cache::broadcast_map_get(&request.stream_id).is_some()
        {
            api_serv::broadcast_stop_with_reason(
                BroadcastStopModel {
                    broadcast_id: request.stream_id.clone(),
                },
                if request.reason == "manual_stop" {
                    "manual_stop"
                } else {
                    "session_close"
                },
            )
            .await
            .map(|_| DeviceStreamState::Stopped)
            .map_err(device_error)
        } else if !force {
            match crate::state::session::Cache::stream_map_release_token(
                &request.stream_id,
                &request.subscription_id,
            ) {
                Some(remaining) if remaining > 0 => Ok(DeviceStreamState::Running),
                Some(_) => {
                    stream_close::begin_with_reason(
                        request.stream_id.clone(),
                        "last_subscription_released",
                    );
                    Ok(DeviceStreamState::Stopped)
                }
                None => Ok(DeviceStreamState::Stopped),
            }
        } else if request.expected_session.is_some() {
            match stream_close::begin_manual(request.stream_id.clone()).await {
                Ok(_) => Ok(DeviceStreamState::Stopping),
                Err(err) => {
                    let current = SipDialogSessionRepository::find_by_stream_id(&request.stream_id)
                        .await
                        .map_err(storage_status)?;
                    if current.is_some_and(|dialog| dialog.state == DialogState::Terminated) {
                        Ok(DeviceStreamState::Stopped)
                    } else {
                        Err(device_error(err))
                    }
                }
            }
        } else {
            stream_close::begin_with_reason(request.stream_id.clone(), "session_close");
            Ok(DeviceStreamState::Stopped)
        };
        let response = match state {
            Ok(state) => {
                let mut response = device_response(&request.stream_id, state, None);
                response.subscription_id = request.subscription_id;
                if let Ok(control) = self.inner.lock() {
                    response.session_node_id = control.identity.node_id.clone();
                    response.session_instance_id = control.identity.instance_id.clone();
                }
                response
            }
            Err(error) => error,
        };
        Ok(tonic::Response::new(response))
    }

    async fn list_active_streams(
        &self,
        request: tonic::Request<ListActiveStreamsRequest>,
    ) -> Result<tonic::Response<ListActiveStreamsResponse>, tonic::Status> {
        let mut request = request.into_inner();
        let identity = self.monitor_identity(request.expected_session.as_ref())?;
        trim_active_stream_request(&mut request);
        let limit = if request.limit == 0 {
            20
        } else {
            request.limit
        };
        if limit > 100 {
            return Err(tonic::Status::invalid_argument("limit must be in 1..=100"));
        }
        if !request.state.is_empty()
            && !matches!(
                request.state.as_str(),
                "starting" | "running" | "stopping" | "failed" | "unknown" | "conflict"
            )
        {
            return Err(tonic::Status::invalid_argument(
                "invalid active stream state",
            ));
        }
        let filter = DialogMonitorFilter {
            stream_id: request.stream_id,
            media_node_id: request.stream_node_id,
            device_id: request.device_id,
            channel_id: request.channel_id,
            ssrc: request.ssrc,
            state: String::new(),
        };
        const MAX_SCAN: u32 = 200;
        let candidates = SipDialogSessionRepository::page_active_for_monitor(
            &identity.node_id,
            (!request.after_stream_id.is_empty()).then_some(request.after_stream_id.as_str()),
            MAX_SCAN,
            &filter,
        )
        .await
        .map_err(storage_status)?;
        let exhausted = candidates.len() < MAX_SCAN as usize;
        let last_scanned = candidates.last().map(|dialog| dialog.stream_id.clone());
        let semaphore = Arc::new(base::tokio::sync::Semaphore::new(8));
        let mut probes = base::tokio::task::JoinSet::new();
        for dialog in candidates {
            let identity = identity.clone();
            let semaphore = semaphore.clone();
            probes.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok();
                match base::tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    active_stream_item(&identity, &dialog),
                )
                .await
                {
                    Ok(item) => item,
                    Err(_) => active_stream_item_with_status(
                        &identity,
                        &dialog,
                        (
                            "unknown".to_string(),
                            "unknown".to_string(),
                            false,
                            "stream_rpc_timeout".to_string(),
                            0,
                            vec![],
                            String::new(),
                        ),
                    ),
                }
            });
        }
        let mut items = Vec::with_capacity(limit as usize + 1);
        while let Some(result) = probes.join_next().await {
            let item = result.map_err(|_| tonic::Status::internal("stream probe task failed"))?;
            if request.state.is_empty() || item.state == request.state {
                items.push(item);
            }
        }
        items.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
        let next_after_id = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|item| item.stream_id.clone())
                .unwrap_or_default()
        } else if !exhausted {
            last_scanned.unwrap_or_default()
        } else {
            String::new()
        };
        Ok(tonic::Response::new(ListActiveStreamsResponse {
            items,
            next_after_id,
            server_time_ms: Local::now().timestamp_millis(),
        }))
    }

    async fn list_active_stream_dialogs(
        &self,
        request: tonic::Request<ListActiveStreamDialogsRequest>,
    ) -> Result<tonic::Response<ListActiveStreamDialogsResponse>, tonic::Status> {
        let mut request = request.into_inner();
        let identity = self.monitor_identity(request.expected_session.as_ref())?;
        trim_active_dialog_request(&mut request);
        let page = request.page.max(1);
        let page_size = if request.page_size == 0 {
            20
        } else {
            request.page_size
        };
        if page_size > 100 {
            return Err(tonic::Status::invalid_argument(
                "page_size must be in 1..=100",
            ));
        }
        if !request.dialog_state.is_empty()
            && !matches!(
                request.dialog_state.as_str(),
                "INVITING" | "ESTABLISHED" | "TERMINATING"
            )
        {
            return Err(tonic::Status::invalid_argument(
                "invalid active dialog state",
            ));
        }
        let filter = DialogMonitorFilter {
            stream_id: request.stream_id,
            media_node_id: request.stream_node_id,
            device_id: request.device_id,
            channel_id: request.channel_id,
            ssrc: request.ssrc,
            state: request.dialog_state,
        };
        let (dialogs, total) = SipDialogSessionRepository::page_active_dialogs_for_monitor(
            &identity.node_id,
            page,
            page_size,
            &filter,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(ListActiveStreamDialogsResponse {
            items: dialogs
                .into_iter()
                .map(|dialog| active_dialog_item(&identity, dialog))
                .collect(),
            total,
            page,
            page_size,
            server_time_ms: Local::now().timestamp_millis(),
        }))
    }

    async fn get_active_stream_management(
        &self,
        request: tonic::Request<GetActiveStreamManagementRequest>,
    ) -> Result<tonic::Response<GetActiveStreamManagementResponse>, tonic::Status> {
        let mut request = request.into_inner();
        let identity = self.monitor_identity(request.expected_session.as_ref())?;
        request.stream_id = request.stream_id.trim().to_string();
        if request.stream_id.is_empty() {
            return Err(tonic::Status::invalid_argument("stream_id is required"));
        }
        let dialog = SipDialogSessionRepository::find_by_stream_id(&request.stream_id)
            .await
            .map_err(storage_status)?
            .ok_or_else(|| tonic::Status::not_found("stream dialog not found"))?;
        if dialog.signal_node_id != identity.node_id {
            return Err(tonic::Status::failed_precondition("wrong_session"));
        }
        if matches!(dialog.state, DialogState::Terminated | DialogState::Orphan) {
            return Ok(tonic::Response::new(GetActiveStreamManagementResponse {
                state: ActiveStreamManagementState::Ended as i32,
                active: None,
                ended: Some(history_stream_item(dialog)),
            }));
        }
        Ok(tonic::Response::new(GetActiveStreamManagementResponse {
            state: ActiveStreamManagementState::Active as i32,
            active: Some(active_stream_item(&identity, &dialog).await),
            ended: None,
        }))
    }

    async fn list_stream_history(
        &self,
        request: tonic::Request<ListStreamHistoryRequest>,
    ) -> Result<tonic::Response<ListStreamHistoryResponse>, tonic::Status> {
        let mut request = request.into_inner();
        let identity = self.monitor_identity(request.expected_session.as_ref())?;
        trim_history_request(&mut request);
        let page = request.page.max(1);
        let page_size = if request.page_size == 0 {
            20
        } else {
            request.page_size
        };
        if page_size > 100 {
            return Err(tonic::Status::invalid_argument(
                "page_size must be in 1..=100",
            ));
        }
        if !request.state.is_empty() && !matches!(request.state.as_str(), "TERMINATED" | "ORPHAN") {
            return Err(tonic::Status::invalid_argument(
                "invalid history stream state",
            ));
        }
        let filter = DialogMonitorFilter {
            stream_id: request.stream_id,
            media_node_id: request.stream_node_id,
            device_id: request.device_id,
            channel_id: request.channel_id,
            ssrc: request.ssrc,
            state: request.state,
        };
        let (dialogs, total) = SipDialogSessionRepository::page_history_for_monitor(
            &identity.node_id,
            page,
            page_size,
            &filter,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(ListStreamHistoryResponse {
            items: dialogs.into_iter().map(history_stream_item).collect(),
            total,
            page,
            page_size,
            server_time_ms: Local::now().timestamp_millis(),
        }))
    }

    async fn set_playback_speed(
        &self,
        request: tonic::Request<SetPlaybackSpeedRequest>,
    ) -> Result<tonic::Response<SetPlaybackSpeedResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.set_playback_speed, req:{request:?}");
        let stream_id = request.stream_id.clone();
        let _presence_control = if request.playback_id.is_empty() {
            None
        } else {
            match playback_presence::acquire_control(
                &request.playback_id,
                &stream_id,
                request.expected_generation,
            ) {
                Some(guard) => Some(guard),
                None => {
                    return Ok(tonic::Response::new(SetPlaybackSpeedResponse {
                        accepted: false,
                        error: Some(error(
                            "playback_presence_terminal",
                            "playback presence is closing or expired",
                        )),
                        generation: request.expected_generation,
                    }));
                }
            }
        };
        let result = api_serv::speed(
            PlaySpeedModel {
                streamId: request.stream_id,
                speedRate: request.speed_rate,
            },
            String::new(),
        )
        .await;
        let result = match result {
            Ok(value) if request.playback_id.is_empty() => Ok(value),
            Ok(value) => {
                let rate_milli = (request.speed_rate * 1000.0).round() as i64;
                if SipDialogSessionRepository::cas_ack_playback_control(
                    &stream_id,
                    &request.playback_id,
                    request.expected_generation,
                    None,
                    Some(rate_milli),
                    Some("PLAYING"),
                    None,
                    &operation_id(request.operation.as_ref()),
                )
                .await
                .map_err(storage_status)?
                {
                    hook_serv::clear_playback_pause_deadline(
                        &stream_id,
                        request.expected_generation,
                    );
                    playback_presence::clear(&request.playback_id, &stream_id);
                    Ok(value)
                } else {
                    Err(GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "stale playback generation",
                        |msg| log_error!("{msg}: stream_id={stream_id}"),
                    ))
                }
            }
            Err(err) => Err(err),
        };
        let response = match result {
            Ok(_) => SetPlaybackSpeedResponse {
                accepted: true,
                error: None,
                generation: request.expected_generation.saturating_add(1),
            },
            Err(err) => SetPlaybackSpeedResponse {
                accepted: false,
                error: Some(gmv_nodec::error::global_error_detail(
                    "playback_speed_failed",
                    &err,
                )),
                generation: request.expected_generation,
            },
        };
        Ok(tonic::Response::new(response))
    }

    async fn seek_playback(
        &self,
        request: tonic::Request<SeekPlaybackRequest>,
    ) -> Result<tonic::Response<PlaybackControlResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.seek_playback, req:{request:?}");
        let stream_id = request.stream_id.clone();
        let Some(_presence_control) = playback_presence::acquire_control(
            &request.playback_id,
            &stream_id,
            request.expected_generation,
        ) else {
            return Ok(tonic::Response::new(PlaybackControlResponse {
                accepted: false,
                error: Some(error(
                    "playback_presence_terminal",
                    "playback presence is closing or expired",
                )),
                generation: request.expected_generation,
                acknowledged_position_sec: request.position_sec,
                acknowledged_speed_rate: 0.0,
                state: PlaybackState::Paused as i32,
            }));
        };
        let playback_range =
            SipDialogSessionRepository::find_playback_range(&stream_id, &request.playback_id)
                .await
                .map_err(storage_status)?;
        let result = match playback_range {
            Some((start_sec, end_sec)) => {
                match playback_seek_offset(request.position_sec, start_sec, end_sec) {
                    Ok(seek_second) => {
                        api_serv::seek(
                            PlaySeekModel {
                                streamId: request.stream_id,
                                seekSecond: seek_second,
                            },
                            String::new(),
                        )
                        .await
                    }
                    Err(err) => Err(err),
                }
            }
            None => Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "playback range is unavailable",
                |msg| log_error!("{msg}: stream_id={stream_id}"),
            )),
        };
        let result = match result {
            Ok(value)
                if SipDialogSessionRepository::cas_ack_playback_control(
                    &stream_id,
                    &request.playback_id,
                    request.expected_generation,
                    Some(request.position_sec),
                    None,
                    Some("PLAYING"),
                    None,
                    &operation_id(request.operation.as_ref()),
                )
                .await
                .map_err(storage_status)? =>
            {
                hook_serv::clear_playback_pause_deadline(&stream_id, request.expected_generation);
                playback_presence::clear(&request.playback_id, &stream_id);
                Ok(value)
            }
            Ok(_) => Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "stale playback generation",
                |msg| log_error!("{msg}: stream_id={stream_id}"),
            )),
            Err(err) => Err(err),
        };
        let response = playback_control_response(
            result,
            request.expected_generation,
            request.position_sec,
            PlaybackState::Playing,
        );
        Ok(tonic::Response::new(response))
    }

    async fn set_playback_state(
        &self,
        request: tonic::Request<SetPlaybackStateRequest>,
    ) -> Result<tonic::Response<PlaybackControlResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.set_playback_state, req:{request:?}");
        let state = PlaybackState::try_from(request.state).unwrap_or(PlaybackState::Unspecified);
        if state == PlaybackState::Unspecified {
            return Ok(tonic::Response::new(PlaybackControlResponse {
                accepted: false,
                error: Some(error(
                    "invalid_playback_state",
                    "playback state is required",
                )),
                generation: request.expected_generation,
                acknowledged_position_sec: 0,
                acknowledged_speed_rate: 0.0,
                state: PlaybackState::Unspecified as i32,
            }));
        }
        if state == PlaybackState::Paused && request.subscription_id.is_empty() {
            return Ok(tonic::Response::new(PlaybackControlResponse {
                accepted: false,
                error: Some(error(
                    "invalid_subscription",
                    "subscription_id is required when pausing playback",
                )),
                generation: request.expected_generation,
                acknowledged_position_sec: 0,
                acknowledged_speed_rate: 0.0,
                state: PlaybackState::Unspecified as i32,
            }));
        }
        let Some(_presence_control) = playback_presence::acquire_control(
            &request.playback_id,
            &request.stream_id,
            request.expected_generation,
        ) else {
            return Ok(tonic::Response::new(PlaybackControlResponse {
                accepted: false,
                error: Some(error(
                    "playback_presence_terminal",
                    "playback presence is closing or expired",
                )),
                generation: request.expected_generation,
                acknowledged_position_sec: 0,
                acknowledged_speed_rate: 0.0,
                state: PlaybackState::Paused as i32,
            }));
        };
        let result =
            api_serv::playback_state(&request.stream_id, state == PlaybackState::Paused).await;
        let pause_expire_at = (state == PlaybackState::Paused).then(|| {
            Local::now().naive_local()
                + TimeDelta::seconds(
                    crate::gb::SessionConf::get_session_by_conf().playback_pause_timeout_secs
                        as i64,
                )
        });
        let result = match result {
            Ok(value)
                if SipDialogSessionRepository::cas_ack_playback_control(
                    &request.stream_id,
                    &request.playback_id,
                    request.expected_generation,
                    None,
                    None,
                    Some(if state == PlaybackState::Paused {
                        "PAUSED"
                    } else {
                        "PLAYING"
                    }),
                    pause_expire_at,
                    &operation_id(request.operation.as_ref()),
                )
                .await
                .map_err(storage_status)? =>
            {
                if let Some(expire_at) = pause_expire_at {
                    hook_serv::schedule_playback_pause_deadline(
                        &request.stream_id,
                        request.expected_generation.saturating_add(1),
                        expire_at,
                    );
                    playback_presence::initialize(
                        &request.playback_id,
                        &request.stream_id,
                        &request.subscription_id,
                        request.expected_generation.saturating_add(1),
                    );
                } else {
                    hook_serv::clear_playback_pause_deadline(
                        &request.stream_id,
                        request.expected_generation,
                    );
                    playback_presence::clear(&request.playback_id, &request.stream_id);
                }
                Ok(value)
            }
            Ok(_) => Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "stale playback generation",
                |msg| log_error!("{msg}: stream_id={}", request.stream_id),
            )),
            Err(err) => Err(err),
        };
        let response = playback_control_response(result, request.expected_generation, 0, state);
        Ok(tonic::Response::new(response))
    }

    async fn refresh_playback_presence(
        &self,
        request: tonic::Request<RefreshPlaybackPresenceRequest>,
    ) -> Result<tonic::Response<RefreshPlaybackPresenceResponse>, tonic::Status> {
        let request = request.into_inner();
        let server_time_ms = playback_presence::now_ms();
        let mut items = Vec::with_capacity(request.items.len());
        for item in request.items {
            let lease = SipDialogSessionRepository::find_playback_pause_lease(&item.stream_id)
                .await
                .map_err(storage_status)?;
            let valid = lease.is_some_and(|lease| {
                lease.playback_id == item.playback_id
                    && lease.state == "PAUSED"
                    && lease.generation == item.generation
                    && lease
                        .expire_at
                        .is_some_and(|expire_at| expire_at > Local::now().naive_local())
            });
            let presence_deadline_ms = valid.then(|| {
                playback_presence::refresh(
                    &item.playback_id,
                    &item.stream_id,
                    &item.subscription_id,
                    item.generation,
                    server_time_ms,
                )
            });
            let presence_deadline_ms = presence_deadline_ms.flatten();
            items.push(PlaybackPresenceHeartbeatResult {
                playback_id: item.playback_id,
                stream_id: item.stream_id,
                accepted: presence_deadline_ms.is_some(),
                terminal: presence_deadline_ms.is_none(),
                generation: item.generation,
                presence_deadline_ms,
            });
        }
        Ok(tonic::Response::new(RefreshPlaybackPresenceResponse {
            server_time_ms,
            items,
        }))
    }

    async fn control_ptz(
        &self,
        request: tonic::Request<ControlPtzRequest>,
    ) -> Result<tonic::Response<ControlPtzResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.control_ptz, req:{request:?}");
        let model = ptz_model(&request);
        let response = match api_serv::ptz(model, String::new()).await {
            Ok(_) => ControlPtzResponse {
                accepted: true,
                error: None,
            },
            Err(err) => ControlPtzResponse {
                accepted: false,
                error: Some(gmv_nodec::error::global_error_detail("ptz_failed", &err)),
            },
        };
        Ok(tonic::Response::new(response))
    }

    async fn get_session_config(
        &self,
        _request: tonic::Request<GetSessionConfigRequest>,
    ) -> Result<tonic::Response<GetSessionConfigResponse>, tonic::Status> {
        debug!("session_control.get_session_config, req:<empty>");
        let conf = crate::gb::SessionConf::get_session_by_conf();
        Ok(tonic::Response::new(GetSessionConfigResponse {
            domain: conf.domain,
            domain_id: conf.domain_id,
            wan_ip: conf.wan_ip.to_string(),
            wan_port: u32::from(conf.wan_port),
        }))
    }

    async fn snapshot_image(
        &self,
        request: tonic::Request<SnapshotImageRequest>,
    ) -> Result<tonic::Response<SnapshotImageResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.snapshot_image, req:{request:?}");
        let count = optional_u8(request.count, "snapshot count")?;
        let interval = optional_u8(request.interval, "snapshot interval")?;
        let info = SnapshotImage {
            device_channel_ident: DeviceChannelIdent {
                device_id: request.device_id,
                channel_id: request.channel_id,
            },
            count,
            interval,
        };
        let response = match edge_serv::snapshot_image(info).await {
            Ok(session_id) => SnapshotImageResponse {
                session_id,
                error: None,
            },
            Err(err) => SnapshotImageResponse {
                session_id: String::new(),
                error: Some(gmv_nodec::error::global_error_detail(
                    "snapshot_failed",
                    &err,
                )),
            },
        };
        Ok(tonic::Response::new(response))
    }

    async fn list_gb_devices(
        &self,
        request: tonic::Request<ListGbDevicesRequest>,
    ) -> Result<tonic::Response<ListGbDevicesResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.list_gb_devices, req:{request:?}");
        let session_node_id = self.session_node_id()?;
        let domain_id = request.domain_id.trim().to_string();
        let device_id = request.device_id.trim().to_string();
        let device_name = request.device_name.trim().to_string();
        let registered_only = request.registered_only;
        let total = if domain_id.is_empty() {
            crate::storage::guard_query::GbDeviceView::count(registered_only).await
        } else {
            crate::storage::guard_query::GbDeviceView::count_by_domain(
                &domain_id,
                &device_id,
                &device_name,
                registered_only,
            )
            .await
        }
        .map_err(storage_status)?;
        let page = request.page.max(1);
        let devices = if request.page_size == 0 {
            crate::storage::guard_query::GbDeviceView::list(registered_only).await
        } else if !domain_id.is_empty() {
            let offset = page.saturating_sub(1).saturating_mul(request.page_size);
            crate::storage::guard_query::GbDeviceView::list_page_by_domain(
                &domain_id,
                &device_id,
                &device_name,
                registered_only,
                offset,
                request.page_size,
            )
            .await
        } else {
            let offset = page.saturating_sub(1).saturating_mul(request.page_size);
            crate::storage::guard_query::GbDeviceView::list_page(
                registered_only,
                offset,
                request.page_size,
            )
            .await
        }
        .map_err(storage_status)?
        .into_iter()
        .map(|device| gb_device_proto(device, &session_node_id))
        .collect();
        Ok(tonic::Response::new(ListGbDevicesResponse {
            devices,
            total,
            page,
            page_size: request.page_size,
        }))
    }

    async fn get_gb_device(
        &self,
        request: tonic::Request<GetGbDeviceRequest>,
    ) -> Result<tonic::Response<GetGbDeviceResponse>, tonic::Status> {
        let session_node_id = self.session_node_id()?;
        let request = request.into_inner();
        debug!("session_control.get_gb_device, req:{request:?}");
        let device = crate::storage::guard_query::GbDeviceView::get(&request.device_id)
            .await
            .map_err(storage_status)?
            .map(|device| gb_device_proto(device, &session_node_id));
        Ok(tonic::Response::new(GetGbDeviceResponse { device }))
    }

    async fn create_gb_device(
        &self,
        request: tonic::Request<CreateGbDeviceRequest>,
    ) -> Result<tonic::Response<CreateGbDeviceResponse>, tonic::Status> {
        let session_node_id = self.session_node_id()?;
        let request = request.into_inner();
        if let Some(device) = request.device.as_ref() {
            debug!(
                "session_control.create_gb_device, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, snapshot_to_mode={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
                device.device_id,
                device.session_node_id,
                device.domain_id,
                device.domain,
                device.longitude,
                device.latitude,
                device.address,
                if device.pwd.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
                device.pwd_check,
                device.alias,
                device.status,
                device.heartbeat_sec,
                device.snapshot_to_mode,
                device.tenant_id,
                device.sys_org_code,
                device.create_by,
                device.update_by
            );
        } else {
            debug!("session_control.create_gb_device, req: device=<none>");
        }
        let device = request
            .device
            .ok_or_else(|| tonic::Status::invalid_argument("device is required"))?;
        validate_snapshot_to_mode(device.snapshot_to_mode)?;
        let device = crate::storage::guard_query::GbDeviceView::create(gb_device_create(device))
            .await
            .map_err(storage_status)?;
        Ok(tonic::Response::new(CreateGbDeviceResponse {
            device: Some(gb_device_proto(device, &session_node_id)),
        }))
    }

    async fn update_gb_device(
        &self,
        request: tonic::Request<UpdateGbDeviceRequest>,
    ) -> Result<tonic::Response<UpdateGbDeviceResponse>, tonic::Status> {
        let session_node_id = self.session_node_id()?;
        let request = request.into_inner();
        if let Some(device) = request.device.as_ref() {
            debug!(
                "session_control.update_gb_device, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, snapshot_to_mode={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
                device.device_id,
                device.session_node_id,
                device.domain_id,
                device.domain,
                device.longitude,
                device.latitude,
                device.address,
                if device.pwd.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
                device.pwd_check,
                device.alias,
                device.status,
                device.heartbeat_sec,
                device.snapshot_to_mode,
                device.tenant_id,
                device.sys_org_code,
                device.create_by,
                device.update_by
            );
        } else {
            debug!("session_control.update_gb_device, req: device=<none>");
        }
        let device = request
            .device
            .ok_or_else(|| tonic::Status::invalid_argument("device is required"))?;
        validate_snapshot_to_mode(device.snapshot_to_mode)?;
        let device = crate::storage::guard_query::GbDeviceView::update(gb_device_create(device))
            .await
            .map_err(storage_status)?
            .ok_or_else(|| tonic::Status::not_found("GB28181 device"))?;
        Ok(tonic::Response::new(UpdateGbDeviceResponse {
            device: Some(gb_device_proto(device, &session_node_id)),
        }))
    }

    async fn delete_gb_device(
        &self,
        request: tonic::Request<DeleteGbDeviceRequest>,
    ) -> Result<tonic::Response<DeleteGbDeviceResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.delete_gb_device, req:{request:?}");
        if request.domain_id.trim().is_empty() {
            return Err(tonic::Status::invalid_argument("domain_id is required"));
        }
        let deleted = crate::storage::guard_query::GbDeviceView::delete(
            &request.device_id,
            &request.domain_id,
        )
        .await
        .map_err(storage_status)?;
        if !deleted {
            return Err(tonic::Status::not_found(format!(
                "GB28181 device {}",
                request.device_id
            )));
        }
        Ok(tonic::Response::new(DeleteGbDeviceResponse { deleted }))
    }

    async fn list_gb_channels(
        &self,
        request: tonic::Request<ListGbChannelsRequest>,
    ) -> Result<tonic::Response<ListGbChannelsResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.list_gb_channels, req:{request:?}");
        let covers = crate::storage::guard_query::GbChannelCoverView::list(&request.device_id)
            .await
            .map_err(storage_status)?
            .into_iter()
            .map(|cover| (cover.channel_id, cover.cover_image_id))
            .collect::<HashMap<_, _>>();
        let channels = crate::storage::guard_query::GbChannelView::list(&request.device_id)
            .await
            .map_err(storage_status)?
            .into_iter()
            .map(|channel| {
                let cover_image_id = covers.get(&channel.channel_id).cloned().unwrap_or_default();
                gb_channel_proto(channel, cover_image_id)
            })
            .collect();
        Ok(tonic::Response::new(ListGbChannelsResponse { channels }))
    }

    async fn get_gb_channel(
        &self,
        request: tonic::Request<GetGbChannelRequest>,
    ) -> Result<tonic::Response<GetGbChannelResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.get_gb_channel, req:{request:?}");
        let cover_image_id =
            crate::storage::guard_query::GbChannelCoverView::list(&request.device_id)
                .await
                .map_err(storage_status)?
                .into_iter()
                .find(|cover| cover.channel_id == request.channel_id)
                .map(|cover| cover.cover_image_id)
                .unwrap_or_default();
        let channel = crate::storage::guard_query::GbChannelView::get(
            &request.device_id,
            &request.channel_id,
        )
        .await
        .map_err(storage_status)?
        .map(|channel| gb_channel_proto(channel, cover_image_id));
        Ok(tonic::Response::new(GetGbChannelResponse { channel }))
    }

    async fn update_gb_channel(
        &self,
        request: tonic::Request<UpdateGbChannelRequest>,
    ) -> Result<tonic::Response<UpdateGbChannelResponse>, tonic::Status> {
        let request = request.into_inner();
        if let Some(channel) = request.channel.as_ref() {
            debug!(
                "session_control.update_gb_channel, req: device_id={}, channel_id={}, alias_name={}, snapshot={}, over_pic_id={}, ptz_enable={}, broadcast_enable={}, audio_enable={}, record_enable={}, playback_enable={}, alarm_enable={}, biz_enable={}, sort_no={}",
                channel.device_id,
                channel.channel_id,
                channel.alias_name,
                channel.snapshot,
                channel.over_pic_id,
                channel.ptz_enable,
                channel.broadcast_enable,
                channel.audio_enable,
                channel.record_enable,
                channel.playback_enable,
                channel.alarm_enable,
                channel.biz_enable,
                channel.sort_no,
            );
        } else {
            debug!("session_control.update_gb_channel, req: channel=<none>");
        }
        let channel = request
            .channel
            .ok_or_else(|| tonic::Status::invalid_argument("channel is required"))?;
        let channel = crate::storage::guard_query::GbChannelView::update_config(
            gb_channel_config_update(channel),
        )
        .await
        .map_err(storage_status)?
        .ok_or_else(|| tonic::Status::not_found("GB28181 channel"))?;
        let cover_image_id =
            crate::storage::guard_query::GbChannelCoverView::list(&channel.device_id)
                .await
                .map_err(storage_status)?
                .into_iter()
                .find(|cover| cover.channel_id == channel.channel_id)
                .map(|cover| cover.cover_image_id)
                .unwrap_or_default();
        Ok(tonic::Response::new(UpdateGbChannelResponse {
            channel: Some(gb_channel_proto(channel, cover_image_id)),
        }))
    }

    async fn list_gb_channel_images(
        &self,
        request: tonic::Request<ListGbChannelImagesRequest>,
    ) -> Result<tonic::Response<ListGbChannelImagesResponse>, tonic::Status> {
        let request = request.into_inner();
        let session_node_id = self.session_node_id()?;
        debug!("session_control.list_gb_channel_images, req:{request:?}");
        let page = request.page.max(1);
        let page_size = if request.page_size == 0 {
            12
        } else {
            request.page_size.min(100)
        };
        let start_time = image_query_datetime(request.start_time_ms)?;
        let end_time = image_query_datetime(request.end_time_ms)?;
        if matches!((start_time, end_time), (Some(start), Some(end)) if start > end) {
            return Err(tonic::Status::invalid_argument(
                "start_time_ms must not be after end_time_ms",
            ));
        }
        let (images, total) = crate::storage::guard_query::GbChannelImageView::list(
            &request.device_id,
            &request.channel_id,
            start_time,
            end_time,
            i64::from(page),
            i64::from(page_size),
        )
        .await
        .map_err(storage_status)?;
        let images = images
            .into_iter()
            .map(|image| gb_channel_image_proto(image, &session_node_id))
            .collect();
        Ok(tonic::Response::new(ListGbChannelImagesResponse {
            images,
            total: total.max(0) as u64,
            page,
            page_size,
        }))
    }

    async fn issue_gb_channel_image_access(
        &self,
        request: tonic::Request<IssueGbChannelImageAccessRequest>,
    ) -> Result<tonic::Response<IssueGbChannelImageAccessResponse>, tonic::Status> {
        let request = request.into_inner();
        let issued = crate::http::image::issue_ticket(
            &request.image_id,
            &request.device_id,
            &request.channel_id,
            &request.mode,
        )
        .await?;
        Ok(tonic::Response::new(IssueGbChannelImageAccessResponse {
            url: issued.url,
            expires_at_ms: issued.expires_at_ms,
            content_type: issued.content_type,
            file_name: issued.file_name,
            file_size: issued.file_size,
        }))
    }

    async fn set_gb_channel_cover(
        &self,
        request: tonic::Request<SetGbChannelCoverRequest>,
    ) -> Result<tonic::Response<UpdateGbChannelResponse>, tonic::Status> {
        let request = request.into_inner();
        let image = crate::storage::guard_query::GbChannelImageView::get(
            &request.image_id,
            &request.device_id,
            &request.channel_id,
        )
        .await
        .map_err(storage_status)?
        .ok_or_else(|| tonic::Status::not_found("GB28181 image"))?;
        let channel = crate::storage::guard_query::GbChannelView::set_cover_image(
            &request.device_id,
            &request.channel_id,
            &image.image_id,
        )
        .await
        .map_err(storage_status)?
        .ok_or_else(|| tonic::Status::not_found("GB28181 channel"))?;
        Ok(tonic::Response::new(UpdateGbChannelResponse {
            channel: Some(gb_channel_proto(channel, image.image_id)),
        }))
    }

    async fn get_gb_channel_records(
        &self,
        request: tonic::Request<GetGbChannelRecordsRequest>,
    ) -> Result<tonic::Response<GetGbChannelRecordsResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.get_gb_channel_records, req:{request:?}");
        let state = crate::storage::device_record::RecordState::get_page(
            &request.device_id,
            &request.channel_id,
            Local::now().timestamp_millis(),
            request.start_time_sec,
            request.end_time_sec,
            request.page,
            request.page_size,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(gb_record_state_proto(state)))
    }

    async fn query_gb_channel_records(
        &self,
        request: tonic::Request<QueryGbChannelRecordsRequest>,
    ) -> Result<tonic::Response<GetGbChannelRecordsResponse>, tonic::Status> {
        let request = request.into_inner();
        let request_id = operation_id(request.operation.as_ref());
        debug!(
            "session_control.query_gb_channel_records, req: request_id={}, device_id={}, channel_id={}, start_time_sec={}, end_time_sec={}",
            request_id,
            request.device_id,
            request.channel_id,
            request.start_time_sec,
            request.end_time_sec
        );
        let state = record_query::start(
            request.device_id,
            request.channel_id,
            request_id,
            request.start_time_sec,
            request.end_time_sec,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(gb_record_state_proto(state)))
    }

    async fn list_gb_resources(
        &self,
        request: tonic::Request<ListGbResourcesRequest>,
    ) -> Result<tonic::Response<ListGbResourcesResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!("session_control.list_gb_resources, req:{request:?}");
        let resources = crate::storage::resource::GbResourceView::list(&request.device_id)
            .await
            .map_err(storage_status)?
            .into_iter()
            .map(gb_resource_proto)
            .collect();
        Ok(tonic::Response::new(ListGbResourcesResponse { resources }))
    }

    async fn save_gb_resource_confirmation(
        &self,
        request: tonic::Request<SaveGbResourceConfirmationRequest>,
    ) -> Result<tonic::Response<GbResourceResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!(
            "session_control.save_gb_resource_confirmation, req: device_id={}, resource_id={}, resource_kind={}, owner_scope={}, owner_id={}, confirmed_by={}, request_id={}",
            request.device_id,
            request.resource_id,
            request.resource_kind,
            request.owner_scope,
            request.owner_id,
            request.confirmed_by,
            request.request_id,
        );
        let resource = crate::storage::resource::GbResourceView::save_confirmation(
            crate::storage::resource::ResourceConfirmationInput {
                device_id: request.device_id,
                resource_id: request.resource_id,
                resource_kind: request.resource_kind,
                owner_scope: request.owner_scope,
                owner_id: request.owner_id,
                suggested_enum_id: request.suggested_enum_id,
                source_parent_id: request.source_parent_id,
                confirmed_by: request.confirmed_by,
                remark: request.remark,
            },
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(GbResourceResponse {
            resource: Some(gb_resource_proto(resource)),
        }))
    }

    async fn reset_gb_resource_confirmation(
        &self,
        request: tonic::Request<ResetGbResourceConfirmationRequest>,
    ) -> Result<tonic::Response<GbResourceResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!(
            "session_control.reset_gb_resource_confirmation, req: device_id={}, resource_id={}, confirmed_by={}, request_id={}",
            request.device_id, request.resource_id, request.confirmed_by, request.request_id,
        );
        let resource = crate::storage::resource::GbResourceView::reset_confirmation(
            &request.device_id,
            &request.resource_id,
            &request.confirmed_by,
        )
        .await
        .map_err(storage_status)?;
        Ok(tonic::Response::new(GbResourceResponse {
            resource: Some(gb_resource_proto(resource)),
        }))
    }
}

impl SessionControlRpc {
    fn session_node_id(&self) -> Result<String, tonic::Status> {
        self.inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))
            .map(|control| control.identity.node_id.clone())
    }

    fn monitor_identity(
        &self,
        expected: Option<&NodeIdentity>,
    ) -> Result<NodeIdentity, tonic::Status> {
        let identity = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))?
            .identity
            .clone();
        let expected = expected
            .ok_or_else(|| tonic::Status::invalid_argument("expected_session is required"))?;
        if expected.node_id != identity.node_id || expected.instance_id != identity.instance_id {
            return Err(tonic::Status::failed_precondition("stale_instance"));
        }
        Ok(identity)
    }
}

fn trim_active_stream_request(request: &mut ListActiveStreamsRequest) {
    request.after_stream_id = request.after_stream_id.trim().to_string();
    request.stream_id = request.stream_id.trim().to_string();
    request.stream_node_id = request.stream_node_id.trim().to_string();
    request.device_id = request.device_id.trim().to_string();
    request.channel_id = request.channel_id.trim().to_string();
    request.ssrc = request.ssrc.trim().to_string();
    request.state = request.state.trim().to_ascii_lowercase();
}

fn trim_active_dialog_request(request: &mut ListActiveStreamDialogsRequest) {
    request.stream_id = request.stream_id.trim().to_string();
    request.stream_node_id = request.stream_node_id.trim().to_string();
    request.device_id = request.device_id.trim().to_string();
    request.channel_id = request.channel_id.trim().to_string();
    request.ssrc = request.ssrc.trim().to_string();
    request.dialog_state = request.dialog_state.trim().to_ascii_uppercase();
}

fn trim_history_request(request: &mut ListStreamHistoryRequest) {
    request.stream_id = request.stream_id.trim().to_string();
    request.stream_node_id = request.stream_node_id.trim().to_string();
    request.device_id = request.device_id.trim().to_string();
    request.channel_id = request.channel_id.trim().to_string();
    request.ssrc = request.ssrc.trim().to_string();
    request.state = request.state.trim().to_ascii_uppercase();
}

async fn active_stream_item(
    identity: &NodeIdentity,
    dialog: &SipDialogSession,
) -> ActiveStreamItem {
    let status = match dialog.state {
        DialogState::Inviting => (
            "starting".to_string(),
            "unknown".to_string(),
            false,
            String::new(),
            0,
            vec![],
            String::new(),
        ),
        DialogState::Terminating => (
            "stopping".to_string(),
            "unknown".to_string(),
            false,
            String::new(),
            0,
            vec![],
            String::new(),
        ),
        DialogState::Established => probe_dialog_media(dialog).await,
        DialogState::Terminated | DialogState::Orphan => (
            "unknown".to_string(),
            "stopped".to_string(),
            false,
            "terminal_dialog_excluded".to_string(),
            0,
            vec![],
            String::new(),
        ),
    };
    active_stream_item_with_status(identity, dialog, status)
}

fn active_dialog_item(identity: &NodeIdentity, dialog: SipDialogSession) -> ActiveStreamDialogItem {
    let (requested_profile, effective_profile, verification) = dialog_stream_profiles(&dialog);
    ActiveStreamDialogItem {
        stream_id: dialog.stream_id,
        session_node_id: identity.node_id.clone(),
        session_instance_id: identity.instance_id.clone(),
        stream_node_id: dialog.media_node_id,
        device_id: dialog.device_id,
        channel_id: dialog.channel_id,
        ssrc: dialog.ssrc.unwrap_or_default(),
        dialog_state: dialog.state.to_string(),
        created_at_ms: local_datetime_ms(dialog.created_at),
        established_at_ms: dialog
            .established_at
            .map(local_datetime_ms)
            .unwrap_or_default(),
        started_at_ms: local_datetime_ms(dialog.established_at.unwrap_or(dialog.created_at)),
        session_type: dialog.session_type.to_string(),
        requested_stream_profile: requested_profile,
        effective_stream_profile: effective_profile,
        stream_profile_verification: verification,
    }
}

fn active_stream_item_with_status(
    identity: &NodeIdentity,
    dialog: &SipDialogSession,
    status: (
        String,
        String,
        bool,
        String,
        u32,
        Vec<ActiveStreamViewerFormat>,
        String,
    ),
) -> ActiveStreamItem {
    let (
        state,
        media_state,
        media_ready,
        diagnostic_reason,
        viewer_count,
        viewer_formats,
        output_format,
    ) = status;
    ActiveStreamItem {
        stream_id: dialog.stream_id.clone(),
        session_node_id: identity.node_id.clone(),
        session_instance_id: identity.instance_id.clone(),
        stream_node_id: dialog.media_node_id.clone(),
        device_id: dialog.device_id.clone(),
        channel_id: dialog.channel_id.clone(),
        ssrc: dialog.ssrc.clone().unwrap_or_default(),
        state,
        dialog_state: dialog.state.to_string(),
        media_state,
        media_ready,
        created_at_ms: local_datetime_ms(dialog.created_at),
        established_at_ms: dialog
            .established_at
            .map(local_datetime_ms)
            .unwrap_or_default(),
        started_at_ms: local_datetime_ms(dialog.established_at.unwrap_or(dialog.created_at)),
        diagnostic_reason,
        session_type: dialog.session_type.to_string(),
        viewer_count,
        viewer_formats,
        supported_formats: supported_media_formats(dialog.session_type),
        output_format,
        requested_stream_profile: dialog_profile_value(dialog.requested_stream_profile.as_deref()),
        effective_stream_profile: dialog_profile_value(dialog.effective_stream_profile.as_deref()),
        stream_profile_verification: dialog_verification_value(
            dialog.stream_profile_verification.as_deref(),
        ),
    }
}

fn supported_media_formats(session_type: DialogSessionType) -> Vec<String> {
    let formats: &[&str] = match session_type {
        DialogSessionType::Live => &["flv", "fmp4", "hls", "ll_hls"],
        DialogSessionType::Playback => &["flv", "fmp4", "hls"],
        DialogSessionType::Download => &["flv", "fmp4", "hls", "mp4"],
        DialogSessionType::Broadcast => &[],
    };
    formats.iter().map(|format| (*format).to_string()).collect()
}

async fn probe_dialog_media(
    dialog: &SipDialogSession,
) -> (
    String,
    String,
    bool,
    String,
    u32,
    Vec<ActiveStreamViewerFormat>,
    String,
) {
    let Some(node) = StreamNodeRegistry::get(&dialog.media_node_id) else {
        return (
            "unknown".to_string(),
            "unknown".to_string(),
            false,
            "stream_node_unavailable".to_string(),
            0,
            vec![],
            String::new(),
        );
    };
    if dialog.session_type == DialogSessionType::Broadcast {
        return match stream_rpc::broadcast_online(
            &node,
            dialog
                .parent_stream_id
                .as_deref()
                .unwrap_or(&dialog.stream_id),
            &dialog.stream_id,
        )
        .await
        {
            Ok(true) => (
                "running".to_string(),
                "online".to_string(),
                true,
                String::new(),
                0,
                vec![],
                String::new(),
            ),
            Ok(false) => (
                "unknown".to_string(),
                "stopped".to_string(),
                false,
                "media_not_running".to_string(),
                0,
                vec![],
                String::new(),
            ),
            Err(_) => (
                "unknown".to_string(),
                "unknown".to_string(),
                false,
                "stream_rpc_unavailable".to_string(),
                0,
                vec![],
                String::new(),
            ),
        };
    }
    match stream_rpc::query_stream(&node, &dialog.stream_id).await {
        Ok(response) => {
            let media_state =
                ProtoStreamState::try_from(response.state).unwrap_or(ProtoStreamState::Unspecified);
            let media_ready = response.media_ready;
            let viewer_count = response.viewer_count;
            let output_format = response.primary_output_format;
            let viewer_formats = response
                .viewer_formats
                .into_iter()
                .map(|item| ActiveStreamViewerFormat {
                    media_format: item.media_format,
                    viewer_count: item.viewer_count,
                })
                .collect::<Vec<_>>();
            match media_state {
                ProtoStreamState::Receiving if media_ready => (
                    "running".to_string(),
                    "receiving".to_string(),
                    true,
                    String::new(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
                ProtoStreamState::Failed => (
                    "failed".to_string(),
                    "failed".to_string(),
                    false,
                    "media_failed".to_string(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
                ProtoStreamState::Starting => (
                    "starting".to_string(),
                    "starting".to_string(),
                    media_ready,
                    String::new(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
                ProtoStreamState::Stopping => (
                    "stopping".to_string(),
                    "stopping".to_string(),
                    false,
                    String::new(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
                ProtoStreamState::Receiving => (
                    "unknown".to_string(),
                    "receiving".to_string(),
                    false,
                    "media_not_ready".to_string(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
                ProtoStreamState::Stopped | ProtoStreamState::Unspecified => (
                    "unknown".to_string(),
                    "stopped".to_string(),
                    false,
                    "media_not_running".to_string(),
                    viewer_count,
                    viewer_formats,
                    output_format,
                ),
            }
        }
        Err(_) => (
            "unknown".to_string(),
            "unknown".to_string(),
            false,
            "stream_rpc_unavailable".to_string(),
            0,
            vec![],
            String::new(),
        ),
    }
}

fn terminal_reason_label(reason: &str) -> &'static str {
    match reason {
        "manual_stop" => "手动停止",
        "last_subscription_released" => "最后一个观看连接已释放",
        "peer_bye" => "设备主动结束",
        "invite_cancelled" => "邀请建立前已取消",
        "invite_failed" => "邀请失败",
        "device_offline" => "设备离线",
        "media_stopped" => "媒体流已停止",
        "media_prepare_failed" => "媒体准备失败",
        "invite_timeout" => "邀请超时",
        "linkage_failed" => "链路关联失败",
        "start_commit_failed" => "启动提交失败",
        "close_timeout" => "关闭超时",
        "bye_failed" => "BYE 关闭失败",
        "media_still_receiving" => "设备仍在推流",
        "media_close_unconfirmed" => "媒体资源关闭未确认",
        "recovery_failed" => "会话恢复失败",
        "dialog_expired" => "会话已过期",
        "internal_error" => "内部错误",
        "legacy_unknown" => "历史数据原因未知",
        "session_close" => "Session 服务关闭",
        _ => "未知原因",
    }
}

fn history_stream_item(dialog: SipDialogSession) -> StreamHistoryItem {
    let (requested_profile, effective_profile, verification) = dialog_stream_profiles(&dialog);
    let legacy_terminal_time = dialog.terminated_at.is_none();
    let ended_at = dialog.terminated_at.unwrap_or(dialog.updated_at);
    let started_at = dialog.established_at.unwrap_or(dialog.created_at);
    let terminal_reason = dialog
        .terminal_reason
        .unwrap_or_else(|| "legacy_unknown".to_string());
    let terminal_reason_label = terminal_reason_label(&terminal_reason).to_string();
    StreamHistoryItem {
        stream_id: dialog.stream_id,
        session_node_id: dialog.signal_node_id,
        stream_node_id: dialog.media_node_id,
        device_id: dialog.device_id,
        channel_id: dialog.channel_id,
        ssrc: dialog.ssrc.unwrap_or_default(),
        session_type: dialog.session_type.to_string(),
        state: dialog.state.to_string(),
        created_at_ms: local_datetime_ms(dialog.created_at),
        established_at_ms: dialog
            .established_at
            .map(local_datetime_ms)
            .unwrap_or_default(),
        terminated_at_ms: local_datetime_ms(ended_at),
        duration_ms: (ended_at - started_at).num_milliseconds().max(0),
        terminal_reason,
        terminal_reason_label,
        error_code: dialog.error_code.unwrap_or_default(),
        legacy_terminal_time,
        stop_reason: dialog.stop_reason.unwrap_or_default(),
        requested_stream_profile: requested_profile,
        effective_stream_profile: effective_profile,
        stream_profile_verification: verification,
    }
}

impl SessionControlRpc {
    async fn start_device_stream(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
        stream_type: &str,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        let request = request.into_inner();
        debug!(
            "session_control.start_{stream_type}, req: operation={:?}, device_id={}, channel_id={}, token={}, start_time_sec={}, end_time_sec={}, trans_mode={}, output_type={}, audio_codec={}, broadcast_codec={}, broadcast_sample_rate={}, broadcast_channel_count={}, broadcast_frame_duration_ms={}, expected_session={:?}",
            request.operation,
            request.device_id,
            request.channel_id,
            if request.token.is_empty() {
                "<empty>"
            } else {
                "<redacted>"
            },
            request.start_time_sec,
            request.end_time_sec,
            request.trans_mode,
            request.output_type,
            request.audio_codec,
            request.broadcast_codec,
            request.broadcast_sample_rate,
            request.broadcast_channel_count,
            request.broadcast_frame_duration_ms,
            request.expected_session
        );
        let identity = {
            let control = self
                .inner
                .lock()
                .map_err(|_| tonic::Status::internal("session control lock poisoned"))?;
            if !control.matches_expected(request.expected_session.as_ref()) {
                return Ok(tonic::Response::new(device_response(
                    "",
                    DeviceStreamState::Failed,
                    Some(error("stale_instance", "session instance does not match")),
                )));
            }
            control.identity.clone()
        };
        let token = if request.token.trim().is_empty() {
            operation_token(request.operation.as_ref())
        } else {
            request.token.clone()
        };
        let media_config =
            match custom_media_config(stream_type, &request.output_type, &request.audio_codec) {
                Ok(config) => config,
                Err(detail) => {
                    return Ok(tonic::Response::new(device_response(
                        "",
                        DeviceStreamState::Failed,
                        Some(detail),
                    )));
                }
            };
        let requested_trans_mode = match trans_mode(&request.trans_mode) {
            Ok(mode) => mode,
            Err(detail) => {
                return Ok(tonic::Response::new(device_response(
                    "",
                    DeviceStreamState::Failed,
                    Some(detail),
                )));
            }
        };
        let stream_profile = match live_stream_profile(stream_type, request.video_stream_profile) {
            Ok(profile) => profile,
            Err(detail) => {
                return Ok(tonic::Response::new(device_response(
                    "",
                    DeviceStreamState::Failed,
                    Some(detail),
                )));
            }
        };
        let subscription_id = token.clone();
        let playback_id = if request.playback_id.trim().is_empty() {
            operation_id(request.operation.as_ref())
        } else {
            request.playback_id.clone()
        };
        let mut response = match stream_type {
            "live" => api_serv::play_live(
                PlayLiveModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: requested_trans_mode,
                    custom_media_config: media_config.clone(),
                    stream_profile,
                },
                token,
            )
            .await
            .map(|info| {
                let mut response = stream_response(
                    info.streamId,
                    info.url,
                    info.video_codec.unwrap_or_default(),
                    info.audio_codec.unwrap_or_default(),
                );
                response.requested_stream_profile = video_stream_profile_value(
                    info.requested_stream_profile.unwrap_or(stream_profile),
                );
                response.effective_stream_profile = video_stream_profile_value(
                    info.effective_stream_profile.unwrap_or(stream_profile),
                );
                response.stream_profile_verification = if info.stream_profile_verified {
                    StreamProfileVerification::Confirmed as i32
                } else {
                    StreamProfileVerification::Unverified as i32
                };
                response
            }),
            "playback" => api_serv::play_back(
                PlayBackModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: requested_trans_mode,
                    custom_media_config: media_config,
                    st: request.start_time_sec,
                    et: request.end_time_sec,
                },
                token,
            )
            .await
            .map(|info| {
                stream_response(
                    info.streamId,
                    info.url,
                    info.video_codec.unwrap_or_default(),
                    info.audio_codec.unwrap_or_default(),
                )
            }),
            "download" => api_serv::download(
                PlayBackModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: requested_trans_mode,
                    custom_media_config: media_config,
                    st: request.start_time_sec,
                    et: request.end_time_sec,
                },
                token,
            )
            .await
            .map(|info| {
                stream_response(
                    info.streamId,
                    info.url,
                    info.video_codec.unwrap_or_default(),
                    info.audio_codec.unwrap_or_default(),
                )
            }),
            "broadcast" => api_serv::broadcast_start(
                BroadcastStartModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    broadcast_id: optional_channel(&request.broadcast_id),
                    leg_id: optional_channel(&request.broadcast_leg_id),
                    expected_stream_node_id: optional_channel(&request.expected_stream_node_id),
                    transport: empty_to_none(request.trans_mode.clone()),
                    codec: empty_to_none(request.broadcast_codec.clone()),
                    sample_rate: non_zero(request.broadcast_sample_rate),
                    channel_count: u8_non_zero(request.broadcast_channel_count),
                    frame_duration_ms: u16_non_zero(request.broadcast_frame_duration_ms),
                },
                token,
            )
            .await
            .map(|info| {
                let mut response =
                    stream_response(info.leg_id, info.input_url, String::new(), info.codec);
                response.broadcast_profile = info.profile;
                response
            }),
            _ => Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "unsupported stream type",
                |msg| log_error!("{msg}: {stream_type}"),
            )),
        }
        .unwrap_or_else(device_error);
        if response.state == DeviceStreamState::Running as i32 {
            if stream_type == "playback" {
                if let Err(err) = SipDialogSessionRepository::initialize_playback_control(
                    &response.stream_id,
                    &playback_id,
                    request.start_time_sec,
                    request.end_time_sec,
                )
                .await
                {
                    stream_close::begin(response.stream_id.clone());
                    return Err(storage_status(err));
                }
            }
            response.subscription_id = subscription_id;
            response.session_node_id = identity.node_id;
            response.session_instance_id = identity.instance_id;
            if stream_type == "playback" {
                response.playback_id = playback_id;
            }
        }
        Ok(tonic::Response::new(response))
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionHookRpc;

#[tonic::async_trait]
impl SessionHook for SessionHookRpc {
    async fn handle_hook(
        &self,
        request: tonic::Request<SessionHookRequest>,
    ) -> Result<tonic::Response<SessionHookResponse>, tonic::Status> {
        let request = request.into_inner();
        let event_type = request.event_type.clone();
        info!(
            "session hook rpc inbound: event_type={}, payload_bytes={}, operation={:?}",
            event_type,
            request.payload_json.len(),
            request.operation
        );
        let response = match event_type.as_str() {
            "stream.registered" | "stream.register" => {
                let value: RegisterStreamInfo = decode_payload(&request.payload_json)?;
                hook_serv::stream_register(value).await;
                hook_response(true, None::<()>)?
            }
            "stream.input_timeout" => {
                let value: StreamState = decode_payload(&request.payload_json)?;
                hook_response(true, Some(hook_serv::stream_input_timeout(value).await))?
            }
            "stream.on_play" | "stream.on_played" => {
                let value: StreamPlayInfo = decode_payload(&request.payload_json)?;
                hook_response(hook_serv::on_play(value), None::<()>)?
            }
            "stream.off_play" => {
                let value: StreamPlayInfo = decode_payload(&request.payload_json)?;
                hook_serv::off_play(value).await;
                hook_response(true, None::<()>)?
            }
            "stream.idle" => {
                let value: OutputStreamInfo = decode_payload(&request.payload_json)?;
                hook_response(true, Some(hook_serv::stream_idle(value).await))?
            }
            "stream.unknown" => {
                let value: UnknownStreamEvent = decode_payload(&request.payload_json)?;
                hook_response(hook_serv::stream_unknown(value).await, None::<()>)?
            }
            "stream.end_record" => {
                let value: StreamRecordInfo = decode_payload(&request.payload_json)?;
                match hook_serv::end_record(value).await {
                    Ok(()) => hook_response(true, None::<()>)?,
                    Err(err) => SessionHookResponse {
                        accepted: false,
                        payload_json: vec![],
                        error: Some(gmv_nodec::error::global_error_detail(
                            "end_record_failed",
                            &err,
                        )),
                    },
                }
            }
            "stream.broadcast_closed" => {
                let value: BroadcastClosedEvent = decode_payload(&request.payload_json)?;
                hook_response(hook_serv::broadcast_closed(value).await, None::<()>)?
            }
            _ => SessionHookResponse {
                accepted: false,
                payload_json: vec![],
                error: Some(error("unknown_hook", "unsupported session hook event_type")),
            },
        };
        info!(
            "session hook rpc outbound: event_type={}, accepted={}, error={:?}, payload_bytes={}",
            event_type,
            response.accepted,
            response.error,
            response.payload_json.len()
        );
        publish_guard_event(
            &format!("{event_type}.handled"),
            format!(
                "event_type={event_type};accepted={};error={:?};payload_bytes={}",
                response.accepted,
                response.error,
                response.payload_json.len()
            )
            .into_bytes(),
        );
        Ok(tonic::Response::new(response))
    }
}

fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, tonic::Status> {
    base::serde_json::from_slice(payload)
        .map_err(|error| tonic::Status::invalid_argument(format!("invalid hook payload: {error}")))
}

fn hook_response<T: base::serde::Serialize>(
    accepted: bool,
    payload: Option<T>,
) -> Result<SessionHookResponse, tonic::Status> {
    let payload_json = match payload {
        Some(value) => base::serde_json::to_vec(&value).map_err(|error| {
            tonic::Status::internal(format!("encode hook response failed: {error}"))
        })?,
        None => vec![],
    };
    Ok(SessionHookResponse {
        accepted,
        payload_json,
        error: None,
    })
}

#[derive(Debug, Clone)]
pub struct SessionControlAdapter {
    identity: NodeIdentity,
    active_streams: HashMap<String, SessionStream>,
    ptz_commands: u64,
}

#[derive(Debug, Clone)]
struct SessionStream {
    device_id: String,
    channel_id: String,
    route_id: String,
    lease_id: String,
    subscriptions: HashSet<String>,
    state: DeviceStreamState,
}

impl SessionControlAdapter {
    pub fn new(identity: NodeIdentity) -> Self {
        Self {
            identity,
            active_streams: HashMap::new(),
            ptz_commands: 0,
        }
    }

    pub fn allocate_stream_request(
        &self,
        operation_id: &str,
        stream_id: &str,
        stream_type: &str,
        device_id: &str,
        channel_id: &str,
    ) -> AllocateStreamRequest {
        AllocateStreamRequest {
            operation: Some(operation(operation_id)),
            stream_id: stream_id.to_string(),
            stream_type: stream_type.to_string(),
            constraints: HashMap::from([
                ("device_id".to_string(), device_id.to_string()),
                ("channel_id".to_string(), channel_id.to_string()),
            ]),
        }
    }

    pub fn stream_start_request(
        &self,
        operation_id: &str,
        stream_id: &str,
        allocation: &AllocateStreamResponse,
    ) -> StartReceiveRequest {
        StartReceiveRequest {
            operation: Some(operation(operation_id)),
            stream_id: stream_id.to_string(),
            route_id: allocation.route_id.clone(),
            lease_id: allocation.lease_id.clone(),
            expected_stream: allocation.stream_node.clone(),
            preferred_endpoints: allocation.endpoints.clone(),
            constraints: HashMap::new(),
            reservation_ttl_ms: 0,
            media_transport: gmv_protocol::stream::v1::MediaTransport::Udp as i32,
        }
    }

    pub fn complete_start_live(
        &mut self,
        request: StartDeviceStreamRequest,
        stream_start: StartReceiveResponse,
    ) -> DeviceStreamResponse {
        if !self.matches_expected(request.expected_session.as_ref()) {
            return device_response(
                "",
                DeviceStreamState::Failed,
                Some(error("stale_instance", "session instance does not match")),
            );
        }
        if stream_start.state != ProtoStreamState::Receiving as i32 {
            return device_response(
                &stream_start.stream_id,
                DeviceStreamState::Failed,
                stream_start.error,
            );
        }
        let stream_id = stream_start.stream_id;
        let subscription_id = request.token.clone();
        self.active_streams
            .entry(stream_id.clone())
            .and_modify(|stream| {
                stream.subscriptions.insert(subscription_id.clone());
            })
            .or_insert(SessionStream {
                device_id: request.device_id,
                channel_id: request.channel_id,
                route_id: request.route_id,
                lease_id: request.lease_id,
                subscriptions: HashSet::from([subscription_id.clone()]),
                state: DeviceStreamState::Running,
            });
        let mut response = device_response(&stream_id, DeviceStreamState::Running, None);
        response.subscription_id = subscription_id;
        response.session_node_id = self.identity.node_id.clone();
        response.session_instance_id = self.identity.instance_id.clone();
        response
    }

    pub fn start_device_stream(
        &mut self,
        request: StartDeviceStreamRequest,
        stream_type: &str,
    ) -> DeviceStreamResponse {
        if !self.matches_expected(request.expected_session.as_ref()) {
            return device_response(
                "",
                DeviceStreamState::Failed,
                Some(error("stale_instance", "session instance does not match")),
            );
        }
        if request.route_id.is_empty() || request.lease_id.is_empty() {
            return device_response(
                "",
                DeviceStreamState::Failed,
                Some(error("invalid_route", "route_id and lease_id are required")),
            );
        }
        let stream_id = stream_id_for(stream_type, &request);
        if let Some(existing) = self.active_streams.get_mut(&stream_id) {
            if existing.lease_id == request.lease_id {
                existing.subscriptions.insert(request.token.clone());
                let mut response = device_response(&stream_id, existing.state, None);
                response.subscription_id = request.token;
                response.session_node_id = self.identity.node_id.clone();
                response.session_instance_id = self.identity.instance_id.clone();
                return response;
            }
            return device_response(
                &stream_id,
                DeviceStreamState::Failed,
                Some(error(
                    "idempotency_conflict",
                    "device stream already has a different lease",
                )),
            );
        }
        self.active_streams.insert(
            stream_id.clone(),
            SessionStream {
                device_id: request.device_id,
                channel_id: request.channel_id,
                route_id: request.route_id,
                lease_id: request.lease_id,
                subscriptions: HashSet::from([request.token.clone()]),
                state: DeviceStreamState::Running,
            },
        );
        let mut response = device_response(&stream_id, DeviceStreamState::Running, None);
        response.subscription_id = request.token;
        response.session_node_id = self.identity.node_id.clone();
        response.session_instance_id = self.identity.instance_id.clone();
        response
    }

    pub fn stop_device_stream(&mut self, request: StopDeviceStreamRequest) -> DeviceStreamResponse {
        match self.active_streams.get_mut(&request.stream_id) {
            Some(stream) => {
                let force = request.force || request.subscription_id.is_empty();
                if force {
                    stream.subscriptions.clear();
                } else {
                    stream.subscriptions.remove(&request.subscription_id);
                }
                stream.state = if stream.subscriptions.is_empty() {
                    DeviceStreamState::Stopped
                } else {
                    DeviceStreamState::Running
                };
                let mut response = device_response(&request.stream_id, stream.state, None);
                response.subscription_id = request.subscription_id;
                response.session_node_id = self.identity.node_id.clone();
                response.session_instance_id = self.identity.instance_id.clone();
                response
            }
            None => device_response(&request.stream_id, DeviceStreamState::Stopped, None),
        }
    }

    pub fn control_ptz(&mut self, request: ControlPtzRequest) -> ControlPtzResponse {
        if request.device_id.is_empty()
            || request.channel_id.is_empty()
            || request.command.is_empty()
        {
            return ControlPtzResponse {
                accepted: false,
                error: Some(error(
                    "invalid_ptz",
                    "device_id, channel_id and command are required",
                )),
            };
        }
        self.ptz_commands += 1;
        ControlPtzResponse {
            accepted: true,
            error: None,
        }
    }

    pub fn resource_snapshot(&self) -> NodeResourceSnapshot {
        NodeResourceSnapshot {
            full: true,
            resources: self
                .active_streams
                .iter()
                .map(|(stream_id, stream)| ResourceReport {
                    resource: Some(ResourceRef {
                        resource_id: stream_id.clone(),
                        resource_type: "device_stream".to_string(),
                    }),
                    state: match stream.state {
                        DeviceStreamState::Running => ResourceState::Running as i32,
                        DeviceStreamState::Stopping => ResourceState::Stopping as i32,
                        DeviceStreamState::Stopped => ResourceState::Stopped as i32,
                        DeviceStreamState::Failed => ResourceState::Failed as i32,
                        _ => ResourceState::Starting as i32,
                    },
                    labels: HashMap::from([
                        ("device_id".to_string(), stream.device_id.clone()),
                        ("channel_id".to_string(), stream.channel_id.clone()),
                        ("route_id".to_string(), stream.route_id.clone()),
                        ("lease_id".to_string(), stream.lease_id.clone()),
                    ]),
                })
                .collect(),
        }
    }

    pub fn guard_unavailable_event(
        &self,
        operation_id: &str,
        stream_id: &str,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence: 1,
            sent_at_epoch_ms: 0,
            payload: Some(node_to_guard_message::Payload::Event(NodeEvent {
                event_id: format!("guard-unavailable-{operation_id}"),
                topic: "session.guard.unavailable".to_string(),
                priority: EventPriority::P1 as i32,
                payload: format!("stream_id={stream_id};guard=unavailable").into_bytes(),
            })),
        }
    }

    fn matches_expected(&self, expected: Option<&NodeIdentity>) -> bool {
        expected
            .map(|expected| {
                expected.node_id == self.identity.node_id
                    && expected.instance_id == self.identity.instance_id
            })
            .unwrap_or(true)
    }
}

fn stream_id_for(stream_type: &str, request: &StartDeviceStreamRequest) -> String {
    if let Some(operation) = &request.operation {
        if !operation.idempotency_key.is_empty() {
            return format!("{stream_type}-{}", operation.idempotency_key);
        }
        if !operation.operation_id.is_empty() {
            return format!("{stream_type}-{}", operation.operation_id);
        }
    }
    format!("{stream_type}-{}-{}", request.device_id, request.channel_id)
}

fn device_response(
    stream_id: &str,
    state: DeviceStreamState,
    error: Option<ErrorDetail>,
) -> DeviceStreamResponse {
    DeviceStreamResponse {
        stream_id: stream_id.to_string(),
        state: state as i32,
        error,
        endpoint: String::new(),
        video_codec: String::new(),
        audio_codec: String::new(),
        subscription_id: String::new(),
        session_node_id: String::new(),
        session_instance_id: String::new(),
        playback_id: String::new(),
        playback_generation: 0,
        broadcast_profile: String::new(),
        requested_stream_profile: VideoStreamProfile::Unspecified as i32,
        effective_stream_profile: VideoStreamProfile::Unspecified as i32,
        stream_profile_verification: StreamProfileVerification::Unspecified as i32,
    }
}

fn playback_control_response(
    result: GlobalResult<bool>,
    generation: u64,
    position_sec: u32,
    state: PlaybackState,
) -> PlaybackControlResponse {
    match result {
        Ok(_) => PlaybackControlResponse {
            accepted: true,
            error: None,
            generation: generation.saturating_add(1),
            acknowledged_position_sec: position_sec,
            acknowledged_speed_rate: 0.0,
            state: state as i32,
        },
        Err(err) => PlaybackControlResponse {
            accepted: false,
            error: Some(gmv_nodec::error::global_error_detail(
                "playback_control_failed",
                &err,
            )),
            generation,
            acknowledged_position_sec: 0,
            acknowledged_speed_rate: 0.0,
            state: PlaybackState::Unspecified as i32,
        },
    }
}

fn playback_seek_offset(position_sec: u32, start_sec: u32, end_sec: u32) -> GlobalResult<u32> {
    if start_sec == 0 || start_sec >= end_sec || position_sec < start_sec || position_sec > end_sec
    {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "playback seek position is outside the selected range",
            |msg| {
                log_error!(
                    "{msg}: position_sec={position_sec}; start_sec={start_sec}; end_sec={end_sec}"
                )
            },
        ));
    }
    Ok(position_sec - start_sec)
}

fn gb_device_proto(
    row: crate::storage::guard_query::GbDeviceView,
    session_node_id: &str,
) -> GbDevice {
    GbDevice {
        device_id: row.device_id,
        session_node_id: session_node_id.to_string(),
        domain_id: row.domain_id,
        domain: row.domain,
        longitude: row.longitude.unwrap_or_default(),
        latitude: row.latitude.unwrap_or_default(),
        address: row.address.unwrap_or_default(),
        pwd: row.pwd.unwrap_or_default(),
        pwd_check: row.pwd_check,
        alias: row.alias.unwrap_or_default(),
        status: row.status,
        heartbeat_sec: row.heartbeat_sec,
        del: row.del,
        create_time: datetime_string(row.create_time),
        tenant_id: row.tenant_id.unwrap_or_default(),
        sys_org_code: row.sys_org_code.unwrap_or_default(),
        create_by: row.create_by.unwrap_or_default(),
        update_by: row.update_by.unwrap_or_default(),
        update_time: datetime_string(row.update_time),
        monitor_status: row.monitor_status,
        device_type: row.device_type.unwrap_or_default(),
        manufacturer: row.manufacturer.unwrap_or_default(),
        model: row.model.unwrap_or_default(),
        firmware: row.firmware.unwrap_or_default(),
        gb_version: row.gb_version.unwrap_or_default(),
        max_camera: row.max_camera,
        camera_in_count: row.camera_in_count,
        camera_off_count: row.camera_off_count,
        register_time: datetime_string(row.register_time),
        snapshot_to_mode: row.snapshot_to_mode,
    }
}

fn gb_device_create(device: GbDevice) -> crate::storage::guard_query::GbDeviceCreate {
    crate::storage::guard_query::GbDeviceCreate {
        device_id: device.device_id,
        domain_id: device.domain_id,
        domain: device.domain,
        longitude: device.longitude,
        latitude: device.latitude,
        address: device.address,
        pwd: device.pwd,
        pwd_check: device.pwd_check,
        alias: device.alias,
        status: device.status,
        heartbeat_sec: device.heartbeat_sec,
        snapshot_to_mode: device.snapshot_to_mode,
        tenant_id: device.tenant_id,
        sys_org_code: device.sys_org_code,
        create_by: device.create_by,
        update_by: device.update_by,
    }
}

fn validate_snapshot_to_mode(value: i64) -> Result<(), tonic::Status> {
    if matches!(value, 0 | 1) {
        Ok(())
    } else {
        Err(tonic::Status::invalid_argument(
            "snapshot_to_mode must be 0 (signaling_peer) or 1 (business_target)",
        ))
    }
}

fn gb_channel_proto(
    row: crate::storage::guard_query::GbChannelView,
    cover_image_id: String,
) -> GbChannel {
    GbChannel {
        device_id: row.device_id,
        channel_id: row.channel_id,
        name: row.name,
        manufacturer: row.manufacturer,
        model: row.model,
        owner: row.owner,
        status: row.status,
        civil_code: row.civil_code,
        address: row.address,
        parent_id: row.parent_id,
        ip_address: row.ip_address,
        port: row.port,
        longitude: row.longitude,
        latitude: row.latitude,
        ptz_type: row.ptz_type,
        alias_name: row.alias_name,
        pic_url: row.pic_url,
        snapshot: row.snapshot,
        over_pic_id: row.over_pic_id,
        ptz_enable: row.ptz_enable,
        broadcast_enable: row.broadcast_enable,
        audio_enable: row.audio_enable,
        record_enable: row.record_enable,
        playback_enable: row.playback_enable,
        alarm_enable: row.alarm_enable,
        biz_enable: row.biz_enable,
        sort_no: row.sort_no,
        created_at_ms: datetime_ms(row.created_at),
        updated_at_ms: datetime_ms(row.updated_at),
        cover_image_id,
    }
}

fn gb_channel_config_update(
    channel: GbChannel,
) -> crate::storage::guard_query::GbChannelConfigUpdate {
    crate::storage::guard_query::GbChannelConfigUpdate {
        device_id: channel.device_id,
        channel_id: channel.channel_id,
        alias_name: channel.alias_name,
        snapshot: channel.snapshot,
        over_pic_id: channel.over_pic_id,
        ptz_enable: channel.ptz_enable,
        broadcast_enable: channel.broadcast_enable,
        audio_enable: channel.audio_enable,
        record_enable: channel.record_enable,
        playback_enable: channel.playback_enable,
        alarm_enable: channel.alarm_enable,
        biz_enable: channel.biz_enable,
        sort_no: channel.sort_no,
    }
}

fn gb_channel_image_proto(
    row: crate::storage::guard_query::GbChannelImageView,
    session_node_id: &str,
) -> GbChannelImage {
    let content_type = crate::http::image::image_content_type(&row.file_format)
        .unwrap_or_default()
        .to_string();
    let file_name = crate::http::image::image_file_name(&row).unwrap_or_default();
    GbChannelImage {
        image_id: row.image_id,
        device_id: row.device_id,
        channel_id: row.channel_id,
        image_url: String::new(),
        created_at_ms: datetime_ms(row.created_at),
        file_name,
        content_type: content_type.clone(),
        file_size: u64::try_from(row.file_size).unwrap_or_default(),
        can_preview: !content_type.is_empty(),
        session_node_id: session_node_id.to_string(),
    }
}

fn gb_record_state_proto(
    state: crate::storage::device_record::RecordState,
) -> GetGbChannelRecordsResponse {
    GetGbChannelRecordsResponse {
        current_batch: state.current_batch.map(gb_record_batch_proto),
        attempt_batch: state.attempt_batch.map(gb_record_batch_proto),
        segments: state
            .segments
            .into_iter()
            .map(gb_record_segment_proto)
            .collect(),
        next_query_at_ms: state.next_query_at_ms,
        server_time_ms: state.server_time_ms,
        total: state.total,
        page: state.page,
        page_size: state.page_size,
    }
}

fn gb_record_batch_proto(
    batch: crate::storage::device_record::RecordQueryBatch,
) -> GbRecordQueryBatch {
    let status = batch.status_name().to_string();
    GbRecordQueryBatch {
        batch_id: batch.batch_id,
        status,
        start_time_sec: batch.start_time_sec,
        end_time_sec: batch.end_time_sec,
        created_at_ms: batch.created_at_ms,
    }
}

fn gb_record_segment_proto(
    segment: crate::storage::device_record::RecordSegment,
) -> GbRecordSegment {
    GbRecordSegment {
        segment_id: segment.segment_id,
        batch_id: segment.batch_id,
        device_id: segment.device_id,
        channel_id: segment.channel_id,
        remote_device_id: segment.remote_device_id,
        name: segment.name,
        file_path: segment.file_path,
        address: segment.address,
        start_time_sec: segment.start_time_sec,
        end_time_sec: segment.end_time_sec,
        secrecy: segment.secrecy,
        record_type: segment.record_type,
        recorder_id: segment.recorder_id,
        file_size: segment.file_size,
    }
}

fn gb_resource_proto(row: crate::storage::resource::GbResourceView) -> GbResource {
    GbResource {
        device_id: row.device_id,
        resource_id: row.resource_id,
        name: row.name,
        status: row.status,
        parent_id: row.parent_id,
        type_code: row.type_code,
        enum_id: row.enum_id,
        enum_name: row.enum_name,
        suggested_kind: row.suggested_kind,
        classification_mode: row.classification_mode,
        effective_kind: row.effective_kind,
        effective_owner_scope: row.effective_owner_scope,
        effective_owner_id: row.effective_owner_id,
        warning: row.warning,
        biz_enable: row.biz_enable,
        owner_biz_enable: row.owner_biz_enable,
        supported: row.supported,
        available: row.available,
        unavailable_reason: row.unavailable_reason,
        confirmation: row.confirmation.map(|confirmation| GbResourceConfirmation {
            status: confirmation.status,
            resource_kind: confirmation.resource_kind,
            owner_scope: confirmation.owner_scope,
            owner_id: confirmation.owner_id,
            suggested_enum_id: confirmation.suggested_enum_id,
            source_parent_id: confirmation.source_parent_id,
            confirmed_by: confirmation.confirmed_by,
            confirmed_at_ms: confirmation.confirmed_at_ms,
            remark: confirmation.remark,
        }),
    }
}

fn datetime_string(value: Option<base::chrono::NaiveDateTime>) -> String {
    value
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn datetime_ms(value: Option<base::chrono::NaiveDateTime>) -> i64 {
    value.map(local_datetime_ms).unwrap_or_default()
}

fn image_query_datetime(
    value_ms: i64,
) -> Result<Option<base::chrono::NaiveDateTime>, tonic::Status> {
    if value_ms == 0 {
        return Ok(None);
    }
    Local
        .timestamp_millis_opt(value_ms)
        .single()
        .map(|value| Some(value.naive_local()))
        .ok_or_else(|| tonic::Status::invalid_argument("invalid image query time"))
}

fn local_datetime_ms(value: base::chrono::NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(&value)
        .earliest()
        .map(|value| value.timestamp_millis())
        .unwrap_or_default()
}

fn cloud_recording_proto(
    record: crate::storage::recording::CloudRecording,
) -> CloudRecordingSummary {
    use crate::storage::recording as storage;

    let status = storage::normalized_status(&record).to_string();
    let start_time_sec = storage::epoch_sec(record.st.as_deref());
    let end_time_sec = storage::epoch_sec(record.et.as_deref());
    let requested_duration_sec =
        u64::try_from(end_time_sec.saturating_sub(start_time_sec)).unwrap_or_default();
    let recorded_duration_ms = u64::try_from(record.recorded_duration_ms).unwrap_or_default();
    let current_size_bytes = u64::try_from(record.current_size_bytes).unwrap_or_default();
    let progress_percent = if status == storage::STATUS_COMPLETED {
        100
    } else if requested_duration_sec == 0 {
        0
    } else {
        u32::try_from(
            recorded_duration_ms
                .saturating_div(1_000)
                .saturating_mul(100)
                .saturating_div(requested_duration_sec)
                .min(100),
        )
        .unwrap_or(100)
    };
    let status_value = match status.as_str() {
        storage::STATUS_STARTING => CloudRecordingStatus::Starting,
        storage::STATUS_RUNNING => CloudRecordingStatus::Running,
        storage::STATUS_STOPPING => CloudRecordingStatus::Stopping,
        storage::STATUS_COMPLETED => CloudRecordingStatus::Completed,
        storage::STATUS_STOPPED => CloudRecordingStatus::Stopped,
        storage::STATUS_PARTIAL => CloudRecordingStatus::Partial,
        storage::STATUS_FAILED => CloudRecordingStatus::Failed,
        storage::STATUS_DELETING => CloudRecordingStatus::Deleting,
        storage::STATUS_DELETED => CloudRecordingStatus::Deleted,
        _ => CloudRecordingStatus::Unspecified,
    };
    let file_state = record
        .file_state
        .as_deref()
        .unwrap_or(storage::FILE_NONE)
        .to_string();
    let file_state_value = match file_state.as_str() {
        storage::FILE_NONE => CloudRecordingFileState::None,
        storage::FILE_WRITING => CloudRecordingFileState::Writing,
        storage::FILE_READY => CloudRecordingFileState::Ready,
        storage::FILE_MISSING => CloudRecordingFileState::Missing,
        storage::FILE_DELETED => CloudRecordingFileState::Deleted,
        _ => CloudRecordingFileState::Unspecified,
    };
    let active = matches!(
        status.as_str(),
        storage::STATUS_STARTING | storage::STATUS_RUNNING | storage::STATUS_STOPPING
    );
    let ready = file_state == storage::FILE_READY;
    let terminal = matches!(
        status.as_str(),
        storage::STATUS_COMPLETED
            | storage::STATUS_STOPPED
            | storage::STATUS_PARTIAL
            | storage::STATUS_FAILED
    );
    let updated_at_ms = storage::epoch_ms(record.lt.as_deref());
    let progress_stale = active
        && updated_at_ms > 0
        && Local::now()
            .timestamp_millis()
            .saturating_sub(updated_at_ms)
            > 15_000;
    CloudRecordingSummary {
        task_id: record.task_id,
        request_id: record.request_id.unwrap_or_default(),
        session_node_id: record.session_node_id.unwrap_or_default(),
        device_id: record.device_id,
        channel_id: record.channel_id,
        start_time_sec,
        end_time_sec,
        requested_duration_sec,
        status: status_value as i32,
        file_state: file_state_value as i32,
        progress_percent,
        recorded_duration_ms,
        progress_stale,
        current_size_bytes,
        final_size_bytes: if ready { current_size_bytes } else { 0 },
        file_format: if ready {
            "mp4".to_string()
        } else {
            String::new()
        },
        requested_by: record.user_id.unwrap_or_default(),
        created_at_ms: storage::epoch_ms(record.ct.as_deref()),
        started_at_ms: storage::epoch_ms(record.started_at.as_deref()),
        finished_at_ms: storage::epoch_ms(record.finished_at.as_deref()),
        updated_at_ms,
        error_code: record.error_code.unwrap_or_default(),
        error_message: record.error_message.unwrap_or_default(),
        can_stop: active && status != storage::STATUS_STOPPING,
        can_play: ready,
        can_download: ready,
        can_delete: terminal,
        stream_id: String::new(),
    }
}

fn storage_status(error: GlobalError) -> tonic::Status {
    gmv_nodec::error::global_error_status(&error)
}

pub(crate) fn storage_status_public(error: GlobalError) -> tonic::Status {
    storage_status(error)
}

fn stream_response(
    stream_id: String,
    endpoint: String,
    video_codec: String,
    audio_codec: String,
) -> DeviceStreamResponse {
    DeviceStreamResponse {
        stream_id,
        state: DeviceStreamState::Running as i32,
        error: None,
        endpoint,
        video_codec,
        audio_codec,
        subscription_id: String::new(),
        session_node_id: String::new(),
        session_instance_id: String::new(),
        playback_id: String::new(),
        playback_generation: 0,
        broadcast_profile: String::new(),
        requested_stream_profile: VideoStreamProfile::Unspecified as i32,
        effective_stream_profile: VideoStreamProfile::Unspecified as i32,
        stream_profile_verification: StreamProfileVerification::Unspecified as i32,
    }
}

fn device_error(err: GlobalError) -> DeviceStreamResponse {
    DeviceStreamResponse {
        stream_id: String::new(),
        state: DeviceStreamState::Failed as i32,
        error: Some(gmv_nodec::error::global_error_detail(
            "session_business_failed",
            &err,
        )),
        endpoint: String::new(),
        video_codec: String::new(),
        audio_codec: String::new(),
        subscription_id: String::new(),
        session_node_id: String::new(),
        session_instance_id: String::new(),
        playback_id: String::new(),
        playback_generation: 0,
        broadcast_profile: String::new(),
        requested_stream_profile: VideoStreamProfile::Unspecified as i32,
        effective_stream_profile: VideoStreamProfile::Unspecified as i32,
        stream_profile_verification: StreamProfileVerification::Unspecified as i32,
    }
}

fn operation_id(operation: Option<&OperationRef>) -> String {
    operation
        .map(|operation| operation.operation_id.clone())
        .unwrap_or_default()
}

fn operation_token(operation: Option<&OperationRef>) -> String {
    operation
        .and_then(|operation| {
            (!operation.idempotency_key.is_empty())
                .then(|| operation.idempotency_key.clone())
                .or_else(|| {
                    (!operation.operation_id.is_empty()).then(|| operation.operation_id.clone())
                })
        })
        .map(|value| format!("gmv-{value}"))
        .unwrap_or_else(|| "gmv-rpc".to_string())
}

fn optional_channel(channel_id: &str) -> Option<String> {
    (!channel_id.trim().is_empty()).then(|| channel_id.to_string())
}

fn empty_to_none(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn non_zero(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn u8_non_zero(value: u32) -> Option<u8> {
    u8::try_from(value).ok().filter(|value| *value != 0)
}

fn u16_non_zero(value: u32) -> Option<u16> {
    u16::try_from(value).ok().filter(|value| *value != 0)
}

fn trans_mode(value: &str) -> Result<Option<TransMode>, ErrorDetail> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "udp" => Ok(Some(TransMode::Udp)),
        "tcp_active" => Ok(Some(TransMode::TcpActive)),
        "tcp_passive" => Ok(Some(TransMode::TcpPassive)),
        _ => Err(error(
            "invalid_media_transport",
            "media transport must be udp, tcp_active, or tcp_passive",
        )),
    }
}

fn live_stream_profile(stream_type: &str, value: i32) -> Result<LiveStreamProfile, ErrorDetail> {
    let profile = VideoStreamProfile::try_from(value)
        .map_err(|_| error("invalid_stream_profile", "unknown video stream profile"))?;
    match (stream_type, profile) {
        ("live", VideoStreamProfile::Unspecified | VideoStreamProfile::Main) => {
            Ok(LiveStreamProfile::Main)
        }
        ("live", VideoStreamProfile::Sub) => Ok(LiveStreamProfile::Sub),
        (_, VideoStreamProfile::Unspecified | VideoStreamProfile::Main) => {
            Ok(LiveStreamProfile::Main)
        }
        (_, VideoStreamProfile::Sub) => Err(error(
            "stream_profile_unsupported",
            "video stream profile is only supported for live preview",
        )),
    }
}

fn video_stream_profile_value(profile: LiveStreamProfile) -> i32 {
    match profile {
        LiveStreamProfile::Main => VideoStreamProfile::Main as i32,
        LiveStreamProfile::Sub => VideoStreamProfile::Sub as i32,
    }
}

fn dialog_stream_profiles(dialog: &SipDialogSession) -> (i32, i32, i32) {
    (
        dialog_profile_value(dialog.requested_stream_profile.as_deref()),
        dialog_profile_value(dialog.effective_stream_profile.as_deref()),
        dialog_verification_value(dialog.stream_profile_verification.as_deref()),
    )
}

fn dialog_profile_value(profile: Option<&str>) -> i32 {
    match profile.map(str::to_ascii_lowercase).as_deref() {
        Some("main") => VideoStreamProfile::Main as i32,
        Some("sub") => VideoStreamProfile::Sub as i32,
        _ => VideoStreamProfile::Unspecified as i32,
    }
}

fn dialog_verification_value(verification: Option<&str>) -> i32 {
    match verification.map(str::to_ascii_lowercase).as_deref() {
        Some("confirmed") => StreamProfileVerification::Confirmed as i32,
        Some("unverified") => StreamProfileVerification::Unverified as i32,
        _ => StreamProfileVerification::Unspecified as i32,
    }
}

fn custom_media_config(
    stream_type: &str,
    output_type: &str,
    audio_codec: &str,
) -> Result<Option<crate::state::model::CustomMediaConfig>, ErrorDetail> {
    let output = match output_type.trim().to_ascii_lowercase().as_str() {
        "" => return Ok(None),
        "http_flv" | "flv" => OutputKind::HttpFlv(HttpFlvOutput {
            fmt: Flv::default(),
        }),
        "dash_fmp4" | "fmp4" => OutputKind::DashFmp4(DashFmp4Output {
            fmt: CMaf::default(),
        }),
        "hls" | "hls_fmp4" => OutputKind::HlsFmp4(HlsFmp4Output {
            fmt: CMaf::default(),
            playlist_profile: HlsPlaylistProfile::Standard,
        }),
        "ll_hls" if stream_type == "live" => OutputKind::HlsFmp4(HlsFmp4Output {
            fmt: CMaf::default(),
            playlist_profile: HlsPlaylistProfile::LowLatency,
        }),
        "ll_hls" => {
            return Err(error(
                "OUTPUT_NOT_ALLOWED_FOR_PLAYBACK",
                "ll_hls output is only allowed for live preview",
            ));
        }
        "mp4" if stream_type == "download" => OutputKind::LocalMp4(LocalMp4Output {
            fmt: Mp4::default(),
            path: String::new(),
            token: None,
            file_name: None,
            min_free_bytes: 0,
        }),
        "mp4" => {
            return Err(error(
                "OUTPUT_NOT_ALLOWED_FOR_LIVE",
                "mp4 output is only allowed for finite downloads",
            ));
        }
        _ => {
            return Err(error(
                "UNSUPPORTED_OUTPUT_TYPE",
                "output_type must be flv, fmp4, hls, ll_hls, or mp4",
            ));
        }
    };
    let transcode = match audio_codec.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "aac" => Some(TranscodeConfig {
            audio_codec: Some(OutputAudioCodec::Aac),
        }),
        _ => {
            return Err(error(
                "UNSUPPORTED_AUDIO_TARGET_CODEC",
                "audio_codec must be aac",
            ));
        }
    };
    Ok(Some(crate::state::model::CustomMediaConfig {
        output,
        codec: None,
        transcode,
        filter: Default::default(),
    }))
}

fn optional_u8(value: u32, name: &str) -> Result<Option<u8>, tonic::Status> {
    if value == 0 {
        Ok(None)
    } else {
        u8::try_from(value)
            .map(Some)
            .map_err(|_| tonic::Status::invalid_argument(format!("{name} must fit u8")))
    }
}

fn ptz_model(request: &ControlPtzRequest) -> PtzControlModel {
    let speed = u8::try_from(request.speed).unwrap_or(u8::MAX).max(1);
    let mut model = PtzControlModel::default();
    model.deviceId = request.device_id.clone();
    model.channelId = request.channel_id.clone();
    model.horizonSpeed = speed;
    model.verticalSpeed = speed;
    model.zoomSpeed = speed.min(15);
    match request.command.trim().to_ascii_lowercase().as_str() {
        "left" => model.leftRight = 1,
        "right" => model.leftRight = 2,
        "up" => model.upDown = 1,
        "down" => model.upDown = 2,
        "left_up" => {
            model.leftRight = 1;
            model.upDown = 1;
        }
        "right_up" => {
            model.leftRight = 2;
            model.upDown = 1;
        }
        "left_down" => {
            model.leftRight = 1;
            model.upDown = 2;
        }
        "right_down" => {
            model.leftRight = 2;
            model.upDown = 2;
        }
        "zoom_out" => model.inOut = 1,
        "zoom_in" => model.inOut = 2,
        _ => {}
    }
    model
}

fn error(code: &str, message: &str) -> ErrorDetail {
    gmv_nodec::error::error_detail(code, message)
}

pub fn operation(operation_id: &str) -> OperationRef {
    OperationRef {
        operation_id: operation_id.to_string(),
        idempotency_key: operation_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gmv_protocol::common::v1::Endpoint;
    use gmv_protocol::session::v1::session_hook_server::SessionHook;

    #[test]
    fn snapshot_to_mode_accepts_only_defined_values() {
        assert!(validate_snapshot_to_mode(0).is_ok());
        assert!(validate_snapshot_to_mode(1).is_ok());
        assert!(validate_snapshot_to_mode(-1).is_err());
        assert!(validate_snapshot_to_mode(2).is_err());
    }

    #[test]
    fn allocation_uses_only_concrete_rtp_endpoint_for_sdp() {
        let endpoint = |name: &str, scheme: &str, port: u32, mode: EndpointMode| Endpoint {
            name: name.to_string(),
            scheme: scheme.to_string(),
            host: "127.0.0.1".to_string(),
            port,
            mode: mode as i32,
            labels: HashMap::new(),
        };
        let mut allocation = AllocateStreamResponse {
            lease_id: "lease-1".to_string(),
            route_id: "route-1".to_string(),
            stream_node: Some(NodeIdentity {
                node_id: "stream-1".to_string(),
                instance_id: "instance-1".to_string(),
                kind: NodeKind::Stream as i32,
            }),
            endpoints: vec![
                endpoint("grpc", "grpc", 19082, EndpointMode::Single),
                endpoint("http", "http", 28570, EndpointMode::Single),
                endpoint("rtp", "rtp", 28600, EndpointMode::Multi),
            ],
            ttl_ms: 30_000,
        };
        assert!(stream_node_from_allocation(&allocation).is_err());

        allocation
            .endpoints
            .push(endpoint("rtp", "rtp", 28607, EndpointMode::Single));
        allocation
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.name == "http")
            .unwrap()
            .host = "epimore.cn".to_string();
        allocation
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.name == "rtp" && endpoint.mode == EndpointMode::Single as i32)
            .unwrap()
            .host = "media.epimore.cn".to_string();
        let node = stream_node_from_allocation(&allocation).unwrap();
        assert_eq!(node.pub_host, "media.epimore.cn");
        assert_eq!(node.pub_port, 28607);
    }

    fn cloud_recording(
        status: &str,
        recorded_duration_ms: i64,
    ) -> crate::storage::recording::CloudRecording {
        crate::storage::recording::CloudRecording {
            task_id: "task-1".to_string(),
            request_id: Some("request-1".to_string()),
            session_node_id: Some("session-1".to_string()),
            stream_id: Some("stream-1".to_string()),
            stream_node: Some("stream-node-1".to_string()),
            device_id: "device-1".to_string(),
            channel_id: "channel-1".to_string(),
            user_id: Some("operator".to_string()),
            st: Some("2026-07-22 10:00:00".to_string()),
            et: Some("2026-07-22 10:01:40".to_string()),
            ct: Some("2026-07-22 10:00:00".to_string()),
            state: Some(1),
            status: Some(status.to_string()),
            file_state: Some(crate::storage::recording::FILE_READY.to_string()),
            recorded_duration_ms,
            current_size_bytes: 1_024,
            started_at: Some("2026-07-22 10:00:00".to_string()),
            finished_at: Some("2026-07-22 10:01:39".to_string()),
            lt: Some("2026-07-22 10:01:39".to_string()),
            error_code: None,
            error_message: None,
        }
    }

    #[test]
    fn completed_cloud_recording_reports_one_hundred_percent() {
        let completed = cloud_recording_proto(cloud_recording(
            crate::storage::recording::STATUS_COMPLETED,
            99_000,
        ));
        let partial = cloud_recording_proto(cloud_recording(
            crate::storage::recording::STATUS_PARTIAL,
            99_000,
        ));

        assert_eq!(completed.progress_percent, 100);
        assert_eq!(partial.progress_percent, 99);
    }

    #[test]
    fn playback_seek_uses_npt_offset_from_selected_start() {
        assert_eq!(
            playback_seek_offset(1_784_131_545, 1_784_131_000, 1_784_134_600).unwrap(),
            545
        );
        assert!(playback_seek_offset(1_784_130_999, 1_784_131_000, 1_784_134_600).is_err());
    }

    #[test]
    fn session_hook_rpc_rejects_unknown_event_and_invalid_payload() {
        base::tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let rpc = SessionHookRpc;
                let response = rpc
                    .handle_hook(tonic::Request::new(SessionHookRequest {
                        operation: Some(operation("hook-unknown")),
                        event_type: "stream.not_supported".to_string(),
                        payload_json: vec![],
                    }))
                    .await
                    .unwrap()
                    .into_inner();
                assert!(!response.accepted);
                assert_eq!(response.error.unwrap().code, "unknown_hook");

                let status = rpc
                    .handle_hook(tonic::Request::new(SessionHookRequest {
                        operation: Some(operation("hook-invalid")),
                        event_type: "stream.input_timeout".to_string(),
                        payload_json: b"not-json".to_vec(),
                    }))
                    .await
                    .unwrap_err();
                assert_eq!(status.code(), tonic::Code::InvalidArgument);
            });
    }

    #[test]
    fn session_builds_guard_and_stream_requests_then_records_running_stream() {
        let node = SessionGuardNode::new(
            "session-1",
            "inst-1",
            (true, "session.example.com".to_string(), 443),
        );
        let register = node.register_request(NodeResourceSnapshot {
            resources: vec![],
            full: true,
        });
        assert_eq!(register.identity.unwrap().kind, NodeKind::Session as i32);
        assert!(register.endpoints.iter().any(|endpoint| {
            endpoint.name == "http"
                && endpoint.scheme == "https"
                && endpoint.host == "session.example.com"
                && endpoint.port == 443
        }));
        assert!(register.capabilities.contains(&"device.ptz".to_string()));
        assert!(
            register
                .capabilities
                .contains(&"protocol.gb28181".to_string())
        );
        assert_eq!(
            register.config.get("protocol").map(String::as_str),
            Some("gb28181")
        );
        assert_eq!(
            register.config.get("service").map(String::as_str),
            Some("session-gb28181")
        );

        let mut control = SessionControlAdapter::new(node.identity.clone());
        let allocate = control.allocate_stream_request("op-1", "stream-1", "live", "dev-1", "ch-1");
        assert_eq!(allocate.constraints["device_id"], "dev-1");
        let allocation = AllocateStreamResponse {
            lease_id: "lease-1".to_string(),
            route_id: "route-1".to_string(),
            stream_node: Some(NodeIdentity {
                node_id: "stream-1".to_string(),
                instance_id: "s-inst".to_string(),
                kind: NodeKind::Stream as i32,
            }),
            endpoints: vec![Endpoint {
                name: "rtp".to_string(),
                scheme: "rtp".to_string(),
                host: "127.0.0.1".to_string(),
                port: 30000,
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            }],
            ttl_ms: 30_000,
        };
        let start_receive = control.stream_start_request("op-1", "stream-1", &allocation);
        assert_eq!(start_receive.lease_id, "lease-1");
        let response = control.complete_start_live(
            StartDeviceStreamRequest {
                operation: Some(operation("op-1")),
                device_id: "dev-1".to_string(),
                channel_id: "ch-1".to_string(),
                route_id: allocation.route_id,
                lease_id: allocation.lease_id,
                expected_session: Some(node.identity.clone()),
                token: "viewer-1".to_string(),
                ..Default::default()
            },
            StartReceiveResponse {
                stream_id: "stream-1".to_string(),
                state: ProtoStreamState::Receiving as i32,
                receive_endpoints: vec![],
                error: None,
            },
        );
        assert_eq!(response.state, DeviceStreamState::Running as i32);
        assert_eq!(response.subscription_id, "viewer-1");
        let second = control.complete_start_live(
            StartDeviceStreamRequest {
                operation: Some(operation("op-2")),
                device_id: "dev-1".to_string(),
                channel_id: "ch-1".to_string(),
                route_id: "route-1".to_string(),
                lease_id: "lease-1".to_string(),
                expected_session: Some(node.identity.clone()),
                token: "viewer-2".to_string(),
                ..Default::default()
            },
            StartReceiveResponse {
                stream_id: "stream-1".to_string(),
                state: ProtoStreamState::Receiving as i32,
                receive_endpoints: vec![],
                error: None,
            },
        );
        assert_eq!(second.subscription_id, "viewer-2");
        let first_release = control.stop_device_stream(StopDeviceStreamRequest {
            stream_id: "stream-1".to_string(),
            subscription_id: "viewer-1".to_string(),
            force: false,
            ..Default::default()
        });
        assert_eq!(first_release.state, DeviceStreamState::Running as i32);
        let last_release = control.stop_device_stream(StopDeviceStreamRequest {
            stream_id: "stream-1".to_string(),
            subscription_id: "viewer-2".to_string(),
            force: false,
            ..Default::default()
        });
        assert_eq!(last_release.state, DeviceStreamState::Stopped as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 1);
        assert!(
            control
                .control_ptz(ControlPtzRequest {
                    operation: Some(operation("ptz-1")),
                    device_id: "dev-1".to_string(),
                    channel_id: "ch-1".to_string(),
                    command: "left".to_string(),
                    speed: 3
                })
                .accepted
        );
    }

    #[test]
    fn session_rejects_stale_instance_and_keeps_autonomy_event_for_guard_loss() {
        let node = SessionGuardNode::new(
            "session-1",
            "inst-1",
            (false, "127.0.0.1".to_string(), 18081),
        );
        let mut control = SessionControlAdapter::new(node.identity.clone());
        let stale = NodeIdentity {
            node_id: "session-1".to_string(),
            instance_id: "old".to_string(),
            kind: NodeKind::Session as i32,
        };
        let response = control.complete_start_live(
            StartDeviceStreamRequest {
                operation: Some(operation("op-stale")),
                device_id: "dev-1".to_string(),
                channel_id: "ch-1".to_string(),
                route_id: "route-1".to_string(),
                lease_id: "lease-1".to_string(),
                expected_session: Some(stale),
                ..Default::default()
            },
            StartReceiveResponse {
                stream_id: "stream-1".to_string(),
                state: ProtoStreamState::Receiving as i32,
                receive_endpoints: vec![],
                error: None,
            },
        );
        assert_eq!(response.state, DeviceStreamState::Failed as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 0);
        assert!(matches!(
            control.guard_unavailable_event("op-1", "stream-1").payload,
            Some(node_to_guard_message::Payload::Event(_))
        ));
    }

    #[test]
    fn ll_hls_is_live_only_while_standard_hls_remains_available_for_playback() {
        let live = custom_media_config("live", "ll_hls", "aac")
            .unwrap()
            .unwrap();
        assert!(matches!(
            live.output,
            OutputKind::HlsFmp4(HlsFmp4Output {
                playlist_profile: HlsPlaylistProfile::LowLatency,
                ..
            })
        ));

        let playback = custom_media_config("playback", "hls", "aac")
            .unwrap()
            .unwrap();
        assert!(matches!(
            playback.output,
            OutputKind::HlsFmp4(HlsFmp4Output {
                playlist_profile: HlsPlaylistProfile::Standard,
                ..
            })
        ));

        let error = custom_media_config("playback", "ll_hls", "aac").unwrap_err();
        assert_eq!(error.code, "OUTPUT_NOT_ALLOWED_FOR_PLAYBACK");
    }

    #[test]
    fn supported_media_formats_follow_dialog_session_type() {
        assert_eq!(
            supported_media_formats(DialogSessionType::Live),
            ["flv", "fmp4", "hls", "ll_hls"]
        );
        assert_eq!(
            supported_media_formats(DialogSessionType::Playback),
            ["flv", "fmp4", "hls"]
        );
        assert_eq!(
            supported_media_formats(DialogSessionType::Download),
            ["flv", "fmp4", "hls", "mp4"]
        );
        assert!(supported_media_formats(DialogSessionType::Broadcast).is_empty());
    }

    #[test]
    fn terminal_reason_labels_are_owned_by_session() {
        assert_eq!(terminal_reason_label("manual_stop"), "手动停止");
        assert_eq!(
            terminal_reason_label("media_still_receiving"),
            "设备仍在推流"
        );
        assert_eq!(terminal_reason_label("future_reason"), "未知原因");
    }
}
