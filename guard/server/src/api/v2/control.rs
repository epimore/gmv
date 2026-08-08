use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use gmv_nodec::error::META_GLOBAL_CODE;
use gmv_protocol::avai::v1::avai_control_client::AvaiControlClient;
use gmv_protocol::avai::v1::{AiTaskState, CancelTaskRequest, CreateTaskRequest};
use gmv_protocol::common::v1::{
    EndpointMode, ErrorDetail, NodeIdentity as ProtoIdentity, NodeKind as ProtoNodeKind,
    OperationRef,
};
use gmv_protocol::session::v1::session_control_client::SessionControlClient;
use gmv_protocol::session::v1::{
    CloudRecordingSummary, ControlPtzRequest, CreateCloudRecordingRequest, CreateGbDeviceRequest,
    DeleteCloudRecordingRequest, DeleteGbDeviceRequest, DeviceStreamState, GbChannel, GbDevice,
    GbResource, GetActiveStreamManagementRequest, GetActiveStreamManagementResponse,
    GetCloudRecordingRequest, GetGbChannelRecordsRequest, GetGbChannelRecordsResponse,
    GetGbChannelRequest, GetGbDeviceRequest, GetSessionConfigRequest,
    IssueCloudRecordingAccessRequest, IssueCloudRecordingAccessResponse,
    IssueGbChannelImageAccessRequest, IssueGbChannelImageAccessResponse,
    ListActiveStreamDialogsRequest, ListActiveStreamDialogsResponse, ListActiveStreamsRequest,
    ListActiveStreamsResponse, ListCloudRecordingsRequest, ListGbChannelImagesRequest,
    ListGbChannelImagesResponse, ListGbChannelsRequest, ListGbDevicesRequest,
    ListGbResourcesRequest, ListStreamHistoryRequest, ListStreamHistoryResponse,
    PlaybackPresenceHeartbeat, PlaybackPresenceHeartbeatResult, PlaybackState,
    QueryGbChannelRecordsRequest, RefreshPlaybackPresenceRequest,
    ResetGbResourceConfirmationRequest, SaveGbResourceConfirmationRequest, SeekPlaybackRequest,
    SetGbChannelCoverRequest, SetPlaybackSpeedRequest, SetPlaybackStateRequest,
    SnapshotImageRequest, StartDeviceStreamRequest, StopCloudRecordingRequest,
    StopDeviceStreamRequest, StreamProfileVerification, UpdateGbChannelRequest,
    UpdateGbChannelResponse, UpdateGbDeviceRequest, VideoStreamProfile,
};
use gmv_protocol::stream::v1::stream_control_client::StreamControlClient;
use gmv_protocol::stream::v1::{
    CloseOutputRequest, CreateOutputRequest, GetPlaybackEndpointsRequest, OutputInfo, OutputState,
};
use uuid::Uuid;

use crate::api::v2::model::{
    AiTaskSummary, AiTaskSummaryState, BroadcastOperationSummary, BroadcastTargetSummary,
    StreamOutputState, StreamOutputSummary, StreamSummary, StreamSummaryState,
};
use crate::core::{
    ConnectionState, GmvGuardErrorCode, GuardError, GuardResult, LeaseState, NodeIdentity,
    NodeKind, RouteState, SchedulingState,
};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::store::InMemoryGuardStore;
use crate::store::model::{
    BroadcastOperationRecord, BroadcastTargetRecord, EndpointModeRecord, NodeRecord, RouteRecord,
    StreamSessionOwnerRecord,
};

static BROADCAST_OPERATION_SETUP: LazyLock<base::tokio::sync::Mutex<()>> =
    LazyLock::new(|| base::tokio::sync::Mutex::new(()));

#[derive(Debug, Clone)]
pub struct BusinessControl {
    store: InMemoryGuardStore,
}

#[derive(Debug, Clone, Default)]
pub struct GbSessionConfigSummary {
    pub domain: String,
    pub domain_id: String,
    pub wan_ip: String,
    pub wan_port: u32,
}

#[derive(Debug, Clone, Default)]
pub struct GbDevicePage {
    pub devices: Vec<GbDevice>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceStreamOptions {
    pub session_node_id: String,
    pub token: String,
    pub start_time_sec: u32,
    pub end_time_sec: u32,
    pub trans_mode: String,
    pub output_type: String,
    pub audio_codec: String,
    pub broadcast_codec: String,
    pub broadcast_sample_rate: u32,
    pub broadcast_channel_count: u32,
    pub broadcast_frame_duration_ms: u32,
    pub playback_id: String,
    pub broadcast_id: String,
    pub broadcast_leg_id: String,
    pub expected_stream_node_id: String,
    pub stream_profile: String,
}

#[derive(Debug, Clone)]
pub struct BroadcastTargetOptions {
    pub device_id: String,
    pub channel_id: String,
    pub session_node_id: String,
    pub trans_mode: String,
}

#[derive(Debug, Clone)]
pub struct BroadcastOperationOptions {
    pub token: String,
    pub default_trans_mode: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channel_count: u32,
    pub frame_duration_ms: u32,
    pub targets: Vec<BroadcastTargetOptions>,
}

struct RpcEdge<'a> {
    service: &'a str,
    action: &'a str,
    node_id: &'a str,
    operation_id: &'a str,
    resource_id: &'a str,
    started: Instant,
}

impl<'a> RpcEdge<'a> {
    fn new(
        service: &'a str,
        action: &'a str,
        node_id: &'a str,
        operation_id: &'a str,
        resource_id: &'a str,
    ) -> Self {
        Self {
            service,
            action,
            node_id,
            operation_id,
            resource_id,
            started: Instant::now(),
        }
    }

    fn success(&self) {
        base::log::debug!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=success, elapsed_ms={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis()
        );
    }

    fn transport_error(&self, error: tonic::Status) -> GuardError {
        let global_code = error
            .metadata()
            .get(META_GLOBAL_CODE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=transport_error, elapsed_ms={}, tonic_code={:?}, global_code={}, reason={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            error.code(),
            global_code,
            error.message()
        );
        node_rpc_status(self.service, self.action, error)
    }

    fn response<T>(&self, response: Result<tonic::Response<T>, tonic::Status>) -> GuardResult<T> {
        response
            .map(tonic::Response::into_inner)
            .map_err(|error| self.transport_error(error))
    }

    fn business_rejection(&self, error: &ErrorDetail) {
        let global_code = error
            .metadata
            .get(META_GLOBAL_CODE)
            .map(String::as_str)
            .unwrap_or("");
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=business_rejection, elapsed_ms={}, remote_code={}, global_code={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            error.code,
            global_code
        );
    }

    fn invalid_response(&self, reason: &str) {
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=invalid_response, elapsed_ms={}, reason={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            reason
        );
    }
}

impl BusinessControl {
    pub async fn create_cloud_recording(
        &self,
        request: CreateCloudRecordingRequest,
    ) -> GuardResult<CloudRecordingSummary> {
        let session = self
            .store
            .get_node(&request.session_node_id)
            .ok_or_else(|| {
                GuardError::NotFound(format!("session node {}", request.session_node_id))
            })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {}",
                request.session_node_id
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "create_cloud_recording",
                &session.identity.node_id,
            ));
        }
        let operation_id = request.request_id.clone();
        let resource_id = format!("{}:{}", request.device_id, request.channel_id);
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "create_cloud_recording",
            &session.identity.node_id,
            &operation_id,
            &resource_id,
        );
        let response = edge.response(client.create_cloud_recording(request).await)?;
        let recording = response.recording.ok_or_else(|| {
            edge.invalid_response("empty_cloud_recording");
            GuardError::Conflict("session returned empty cloud recording".to_string())
        })?;
        edge.success();
        Ok(recording)
    }

    pub async fn list_cloud_recordings(
        &self,
        session_node_id: &str,
        request: ListCloudRecordingsRequest,
    ) -> GuardResult<(Vec<CloudRecordingSummary>, u64, u32, u32)> {
        let session = self
            .store
            .get_node(session_node_id)
            .ok_or_else(|| GuardError::NotFound(format!("session node {session_node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "list_cloud_recordings",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let resource_id = request.device_id.clone();
        let edge = RpcEdge::new(
            "session",
            "list_cloud_recordings",
            session_node_id,
            "",
            &resource_id,
        );
        let response = edge.response(client.list_cloud_recordings(request).await)?;
        edge.success();
        Ok((
            response.recordings,
            response.total,
            response.page,
            response.page_size,
        ))
    }

    pub async fn list_active_streams(
        &self,
        session_node_id: &str,
        mut request: ListActiveStreamsRequest,
    ) -> GuardResult<ListActiveStreamsResponse> {
        let session = self.monitor_session_node(session_node_id)?;
        request.expected_session = Some(proto_identity(&session.identity));
        let mut client = self.session_client(&session).await?;
        let resource_id = request.stream_id.clone();
        let edge = RpcEdge::new(
            "session",
            "list_active_streams",
            session_node_id,
            "",
            &resource_id,
        );
        let response = edge.response(client.list_active_streams(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn list_active_stream_dialogs(
        &self,
        session_node_id: &str,
        mut request: ListActiveStreamDialogsRequest,
    ) -> GuardResult<ListActiveStreamDialogsResponse> {
        let session = self.monitor_session_node(session_node_id)?;
        request.expected_session = Some(proto_identity(&session.identity));
        let mut client = self.session_client(&session).await?;
        let resource_id = request.stream_id.clone();
        let edge = RpcEdge::new(
            "session",
            "list_active_stream_dialogs",
            session_node_id,
            "",
            &resource_id,
        );
        let response = edge.response(client.list_active_stream_dialogs(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn get_active_stream_management(
        &self,
        session_node_id: &str,
        stream_id: &str,
    ) -> GuardResult<GetActiveStreamManagementResponse> {
        let session = self.monitor_session_node(session_node_id)?;
        let mut client = self.session_client(&session).await?;
        let request = GetActiveStreamManagementRequest {
            stream_id: stream_id.to_string(),
            expected_session: Some(proto_identity(&session.identity)),
        };
        let edge = RpcEdge::new(
            "session",
            "get_active_stream_management",
            session_node_id,
            "",
            stream_id,
        );
        let response = edge.response(client.get_active_stream_management(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn list_stream_history(
        &self,
        session_node_id: &str,
        mut request: ListStreamHistoryRequest,
    ) -> GuardResult<ListStreamHistoryResponse> {
        let session = self.monitor_session_node(session_node_id)?;
        request.expected_session = Some(proto_identity(&session.identity));
        let mut client = self.session_client(&session).await?;
        let resource_id = request.stream_id.clone();
        let edge = RpcEdge::new(
            "session",
            "list_stream_history",
            session_node_id,
            "",
            &resource_id,
        );
        let response = edge.response(client.list_stream_history(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn stop_monitored_stream(
        &self,
        session_node_id: &str,
        operation_id: &str,
        stream_id: &str,
        stop_reason: &str,
    ) -> GuardResult<gmv_protocol::session::v1::DeviceStreamResponse> {
        let session = self.monitor_session_node(session_node_id)?;
        let mut client = self.session_client(&session).await?;
        let request = StopDeviceStreamRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            reason: "manual_stop".to_string(),
            subscription_id: String::new(),
            force: true,
            expected_session: Some(proto_identity(&session.identity)),
            stop_reason: stop_reason.to_string(),
        };
        let edge = RpcEdge::new(
            "session",
            "stop_monitored_stream",
            session_node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(client.stop_device_stream(request).await)?;
        if let Some(error) = non_empty_error(response.error.clone()) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "stop_monitored_stream",
                error,
                "stream_stop_failed",
                "停止视频流失败，请稍后重试",
                true,
            ));
        }
        if !matches!(
            DeviceStreamState::try_from(response.state),
            Ok(DeviceStreamState::Stopping | DeviceStreamState::Stopped)
        ) {
            edge.invalid_response("stream_not_stopping");
            return Err(GuardError::Conflict(
                "session did not accept stream stop".to_string(),
            ));
        }
        edge.success();
        Ok(response)
    }

    pub async fn get_cloud_recording(&self, task_id: &str) -> GuardResult<CloudRecordingSummary> {
        self.cloud_recording_session(task_id)
            .await
            .map(|value| value.1)
    }

    pub async fn stop_cloud_recording(
        &self,
        task_id: &str,
        request_id: &str,
    ) -> GuardResult<CloudRecordingSummary> {
        let (session, _) = self.cloud_recording_session(task_id).await?;
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "stop_cloud_recording",
            &session.identity.node_id,
            request_id,
            task_id,
        );
        let response = edge.response(
            client
                .stop_cloud_recording(StopCloudRecordingRequest {
                    operation: Some(OperationRef {
                        operation_id: request_id.to_string(),
                        idempotency_key: request_id.to_string(),
                    }),
                    task_id: task_id.to_string(),
                    request_id: request_id.to_string(),
                })
                .await,
        )?;
        edge.success();
        response.recording.ok_or_else(|| {
            GuardError::Conflict("session returned empty cloud recording".to_string())
        })
    }

    pub async fn delete_cloud_recording(
        &self,
        task_id: &str,
        request_id: &str,
    ) -> GuardResult<CloudRecordingSummary> {
        let (session, _) = self.cloud_recording_session(task_id).await?;
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "delete_cloud_recording",
            &session.identity.node_id,
            request_id,
            task_id,
        );
        let response = edge.response(
            client
                .delete_cloud_recording(DeleteCloudRecordingRequest {
                    operation: Some(OperationRef {
                        operation_id: request_id.to_string(),
                        idempotency_key: request_id.to_string(),
                    }),
                    task_id: task_id.to_string(),
                    request_id: request_id.to_string(),
                })
                .await,
        )?;
        edge.success();
        response.recording.ok_or_else(|| {
            GuardError::Conflict("session returned empty cloud recording".to_string())
        })
    }

    pub async fn issue_cloud_recording_access(
        &self,
        task_id: &str,
        operation_id: &str,
        mode: &str,
    ) -> GuardResult<IssueCloudRecordingAccessResponse> {
        let (session, _) = self.cloud_recording_session(task_id).await?;
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "issue_cloud_recording_access",
            &session.identity.node_id,
            operation_id,
            task_id,
        );
        let response = edge.response(
            client
                .issue_cloud_recording_access(IssueCloudRecordingAccessRequest {
                    operation: Some(OperationRef {
                        operation_id: operation_id.to_string(),
                        idempotency_key: String::new(),
                    }),
                    task_id: task_id.to_string(),
                    mode: mode.to_string(),
                })
                .await,
        )?;
        edge.success();
        Ok(response)
    }

    async fn cloud_recording_session(
        &self,
        task_id: &str,
    ) -> GuardResult<(NodeRecord, CloudRecordingSummary)> {
        for session in self.session_nodes() {
            let Ok(mut client) = self.session_client(&session).await else {
                continue;
            };
            match client
                .get_cloud_recording(GetCloudRecordingRequest {
                    task_id: task_id.to_string(),
                })
                .await
            {
                Ok(response) => {
                    if let Some(recording) = response.into_inner().recording {
                        return Ok((session, recording));
                    }
                }
                Err(status) if status.code() == tonic::Code::NotFound => continue,
                Err(status) => {
                    return Err(node_rpc_status("session", "get_cloud_recording", status));
                }
            }
        }
        Err(GuardError::NotFound(format!("cloud recording {task_id}")))
    }

    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    fn gb_session_for_device(&self, device: &GbDevice, action: &str) -> GuardResult<NodeRecord> {
        let node_id = &device.session_node_id;
        let session = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", action, node_id));
        }
        Ok(session)
    }

    pub async fn gb_session_config(&self, node_id: &str) -> GuardResult<GbSessionConfigSummary> {
        let session = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "get_session_config", node_id));
        }
        let mut client = self.session_client(&session).await?;
        let request = GetSessionConfigRequest {};
        base::log::debug!(
            "guard rpc client outbound: method=session_control.get_session_config, node={}, req:<empty>",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "get_session_config",
            &session.identity.node_id,
            "",
            "",
        );
        let response = edge.response(client.get_session_config(request).await)?;
        edge.success();
        Ok(GbSessionConfigSummary {
            domain: response.domain,
            domain_id: response.domain_id,
            wan_ip: response.wan_ip,
            wan_port: response.wan_port,
        })
    }

    pub async fn first_gb_session_node_by_domain(&self) -> GuardResult<(String, String)> {
        let mut options = Vec::new();
        for session in self.session_nodes() {
            if let Ok(config) = self.gb_session_config(&session.identity.node_id).await
                && !config.domain_id.is_empty()
            {
                options.push((session.identity.node_id, config.domain_id));
            }
        }
        options.sort_by(|left, right| left.1.cmp(&right.1));
        options
            .into_iter()
            .next()
            .ok_or_else(|| GuardError::NotFound("GB28181 session node with domain_id".to_string()))
    }

    pub async fn list_gb_devices(&self) -> GuardResult<Vec<GbDevice>> {
        let mut devices = Vec::new();
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = ListGbDevicesRequest {
                page: 0,
                page_size: 0,
                domain_id: String::new(),
                device_id: String::new(),
                device_name: String::new(),
                registered_only: false,
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.list_gb_devices, node={}, req:{request:?}",
                session.identity.node_id,
            );
            let edge = RpcEdge::new(
                "session",
                "list_gb_devices",
                &session.identity.node_id,
                "",
                "",
            );
            let mut response = edge.response(client.list_gb_devices(request).await)?;
            edge.success();
            for device in &mut response.devices {
                if device.session_node_id.is_empty() {
                    device.session_node_id = session.identity.node_id.clone();
                }
            }
            devices.extend(response.devices);
        }
        devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        Ok(devices)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_gb_device_page(
        &self,
        session_node_id: &str,
        domain_id: &str,
        device_id: &str,
        device_name: &str,
        registered_only: bool,
        page: u32,
        page_size: u32,
    ) -> GuardResult<GbDevicePage> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        let page = page.max(1);
        let page_size = page_size.max(1);
        let mut client = self.session_client(&session).await?;
        let request = ListGbDevicesRequest {
            page,
            page_size,
            domain_id: domain_id.to_string(),
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            registered_only,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.list_gb_devices, node={}, req:{request:?}",
            session.identity.node_id,
        );
        let edge = RpcEdge::new(
            "session",
            "list_gb_devices",
            &session.identity.node_id,
            "",
            device_id,
        );
        let mut response = edge.response(client.list_gb_devices(request).await)?;
        edge.success();
        for device in &mut response.devices {
            if device.session_node_id.is_empty() {
                device.session_node_id = session.identity.node_id.clone();
            }
        }
        Ok(GbDevicePage {
            devices: response.devices,
            total: response.total,
            page: response.page,
            page_size: response.page_size,
        })
    }

    pub async fn create_gb_device(&self, mut device: GbDevice) -> GuardResult<GbDevice> {
        let node_id = device.session_node_id.clone();
        let resource_id = device.device_id.clone();
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "create_gb_device", &node_id));
        }
        device.session_node_id.clear();
        base::log::debug!(
            "guard rpc client outbound: method=session_control.create_gb_device, node={}, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
            session.identity.node_id,
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
            device.tenant_id,
            device.sys_org_code,
            device.create_by,
            device.update_by
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "create_gb_device",
            &session.identity.node_id,
            "",
            &resource_id,
        );
        let response = edge.response(
            client
                .create_gb_device(CreateGbDeviceRequest {
                    device: Some(device),
                })
                .await,
        )?;
        let Some(mut response) = response.device else {
            edge.invalid_response("empty_device");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 device".to_string(),
            ));
        };
        edge.success();
        if response.session_node_id.is_empty() {
            response.session_node_id = session.identity.node_id;
        }
        Ok(response)
    }

    pub async fn update_gb_device(&self, mut device: GbDevice) -> GuardResult<GbDevice> {
        let node_id = device.session_node_id.clone();
        let resource_id = device.device_id.clone();
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "update_gb_device", &node_id));
        }
        device.session_node_id.clear();
        base::log::debug!(
            "guard rpc client outbound: method=session_control.update_gb_device, node={}, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
            session.identity.node_id,
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
            device.tenant_id,
            device.sys_org_code,
            device.create_by,
            device.update_by
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "update_gb_device",
            &session.identity.node_id,
            "",
            &resource_id,
        );
        let response = edge.response(
            client
                .update_gb_device(UpdateGbDeviceRequest {
                    device: Some(device),
                })
                .await,
        )?;
        let Some(mut response) = response.device else {
            edge.invalid_response("empty_device");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 device".to_string(),
            ));
        };
        edge.success();
        if response.session_node_id.is_empty() {
            response.session_node_id = session.identity.node_id;
        }
        Ok(response)
    }

    pub async fn delete_gb_device(
        &self,
        session_node_id: &str,
        device_id: &str,
        domain_id: &str,
    ) -> GuardResult<()> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        let request = DeleteGbDeviceRequest {
            device_id: device_id.to_string(),
            domain_id: domain_id.to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.delete_gb_device, node={}, req:{request:?}",
            session.identity.node_id,
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "delete_gb_device",
            &session.identity.node_id,
            "",
            device_id,
        );
        edge.response(client.delete_gb_device(request).await)?;
        edge.success();
        Ok(())
    }

    pub async fn get_gb_device(&self, device_id: &str) -> GuardResult<Option<GbDevice>> {
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = GetGbDeviceRequest {
                device_id: device_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.get_gb_device, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "get_gb_device",
                &session.identity.node_id,
                "",
                device_id,
            );
            let response = edge.response(client.get_gb_device(request).await)?;
            edge.success();
            if let Some(mut device) = response.device {
                if device.session_node_id.is_empty() {
                    device.session_node_id = session.identity.node_id;
                }
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    pub async fn list_gb_channels(&self, device_id: &str) -> GuardResult<Vec<GbChannel>> {
        let mut channels = Vec::new();
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = ListGbChannelsRequest {
                device_id: device_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.list_gb_channels, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "list_gb_channels",
                &session.identity.node_id,
                "",
                device_id,
            );
            let response = edge.response(client.list_gb_channels(request).await)?;
            edge.success();
            channels.extend(response.channels);
        }
        channels.sort_by(|left, right| {
            left.sort_no
                .cmp(&right.sort_no)
                .then_with(|| left.channel_id.cmp(&right.channel_id))
        });
        Ok(channels)
    }

    pub async fn list_gb_channels_for_session(
        &self,
        session_node_id: &str,
        device_id: &str,
    ) -> GuardResult<Vec<GbChannel>> {
        if session_node_id.trim().is_empty() {
            return self.list_gb_channels(device_id).await;
        }
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "list_gb_channels",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let request = ListGbChannelsRequest {
            device_id: device_id.to_string(),
        };
        let edge = RpcEdge::new(
            "session",
            "list_gb_channels",
            &session.identity.node_id,
            "",
            device_id,
        );
        let mut response = edge.response(client.list_gb_channels(request).await)?;
        edge.success();
        response.channels.sort_by(|left, right| {
            left.sort_no
                .cmp(&right.sort_no)
                .then_with(|| left.channel_id.cmp(&right.channel_id))
        });
        Ok(response.channels)
    }

    pub async fn get_gb_channel(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Option<GbChannel>> {
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = GetGbChannelRequest {
                device_id: device_id.to_string(),
                channel_id: channel_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.get_gb_channel, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "get_gb_channel",
                &session.identity.node_id,
                "",
                channel_id,
            );
            let response = edge.response(client.get_gb_channel(request).await)?;
            edge.success();
            if response.channel.is_some() {
                return Ok(response.channel);
            }
        }
        Ok(None)
    }

    pub async fn update_gb_channel(&self, channel: GbChannel) -> GuardResult<GbChannel> {
        let device_id = channel.device_id.clone();
        let channel_id = channel.channel_id.clone();
        let device = self
            .get_gb_device(&device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
        let node_id = device.session_node_id;
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "update_gb_channel", &node_id));
        }
        let request = UpdateGbChannelRequest {
            channel: Some(channel),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.update_gb_channel, node={}, req: device_id={}, channel_id={}",
            session.identity.node_id,
            device_id,
            channel_id,
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "update_gb_channel",
            &session.identity.node_id,
            "",
            &channel_id,
        );
        let response = edge.response(client.update_gb_channel(request).await)?;
        let Some(channel) = response.channel else {
            edge.invalid_response("empty_channel");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 channel".to_string(),
            ));
        };
        edge.success();
        Ok(channel)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_gb_channel_images(
        &self,
        session_node_id: &str,
        device_id: &str,
        channel_id: &str,
        start_time_ms: i64,
        end_time_ms: i64,
        page: u32,
        page_size: u32,
    ) -> GuardResult<ListGbChannelImagesResponse> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "list_gb_channel_images",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let request = ListGbChannelImagesRequest {
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            start_time_ms,
            end_time_ms,
            page,
            page_size,
        };
        let edge = RpcEdge::new(
            "session",
            "list_gb_channel_images",
            &session.identity.node_id,
            "",
            channel_id,
        );
        let response = edge.response(client.list_gb_channel_images(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn issue_gb_channel_image_access(
        &self,
        session_node_id: &str,
        request: IssueGbChannelImageAccessRequest,
    ) -> GuardResult<IssueGbChannelImageAccessResponse> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "issue_gb_channel_image_access",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let operation_id = request
            .operation
            .as_ref()
            .map(|operation| operation.operation_id.clone())
            .unwrap_or_default();
        let resource_id = request.image_id.clone();
        let edge = RpcEdge::new(
            "session",
            "issue_gb_channel_image_access",
            &session.identity.node_id,
            &operation_id,
            &resource_id,
        );
        let response = edge.response(client.issue_gb_channel_image_access(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn set_gb_channel_cover(
        &self,
        session_node_id: &str,
        request: SetGbChannelCoverRequest,
    ) -> GuardResult<UpdateGbChannelResponse> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "set_gb_channel_cover",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let operation_id = request
            .operation
            .as_ref()
            .map(|operation| operation.operation_id.clone())
            .unwrap_or_default();
        let resource_id = request.channel_id.clone();
        let edge = RpcEdge::new(
            "session",
            "set_gb_channel_cover",
            &session.identity.node_id,
            &operation_id,
            &resource_id,
        );
        let response = edge.response(client.set_gb_channel_cover(request).await)?;
        edge.success();
        Ok(response)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_gb_channel_records(
        &self,
        session_node_id: &str,
        device_id: &str,
        channel_id: &str,
        start_time_sec: i64,
        end_time_sec: i64,
        page: u32,
        page_size: u32,
    ) -> GuardResult<GetGbChannelRecordsResponse> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "get_gb_channel_records",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let request = GetGbChannelRecordsRequest {
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            start_time_sec,
            end_time_sec,
            page,
            page_size,
        };
        let edge = RpcEdge::new(
            "session",
            "get_gb_channel_records",
            &session.identity.node_id,
            "",
            channel_id,
        );
        let response = edge.response(client.get_gb_channel_records(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn query_gb_channel_records(
        &self,
        session_node_id: &str,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        start_time_sec: i64,
        end_time_sec: i64,
    ) -> GuardResult<GetGbChannelRecordsResponse> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "query_gb_channel_records",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let request = QueryGbChannelRecordsRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            start_time_sec,
            end_time_sec,
        };
        let edge = RpcEdge::new(
            "session",
            "query_gb_channel_records",
            &session.identity.node_id,
            operation_id,
            channel_id,
        );
        let response = edge.response(client.query_gb_channel_records(request).await)?;
        edge.success();
        Ok(response)
    }

    pub async fn list_gb_resources(&self, device_id: &str) -> GuardResult<Vec<GbResource>> {
        let device = self
            .get_gb_device(device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
        let session = self.gb_session_for_device(&device, "list_gb_resources")?;
        let mut client = self.session_client(&session).await?;
        let request = ListGbResourcesRequest {
            device_id: device_id.to_string(),
        };
        let edge = RpcEdge::new(
            "session",
            "list_gb_resources",
            &session.identity.node_id,
            "",
            device_id,
        );
        let response = edge.response(client.list_gb_resources(request).await)?;
        edge.success();
        Ok(response.resources)
    }

    pub async fn list_gb_resources_for_session(
        &self,
        session_node_id: &str,
        device_id: &str,
    ) -> GuardResult<Vec<GbResource>> {
        if session_node_id.trim().is_empty() {
            return self.list_gb_resources(device_id).await;
        }
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable(
                "session",
                "list_gb_resources",
                session_node_id,
            ));
        }
        let mut client = self.session_client(&session).await?;
        let request = ListGbResourcesRequest {
            device_id: device_id.to_string(),
        };
        let edge = RpcEdge::new(
            "session",
            "list_gb_resources",
            &session.identity.node_id,
            "",
            device_id,
        );
        let response = edge.response(client.list_gb_resources(request).await)?;
        edge.success();
        Ok(response.resources)
    }

    pub async fn save_gb_resource_confirmation(
        &self,
        request: SaveGbResourceConfirmationRequest,
    ) -> GuardResult<GbResource> {
        let device = self
            .get_gb_device(&request.device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {}", request.device_id)))?;
        let session = self.gb_session_for_device(&device, "save_gb_resource_confirmation")?;
        let resource_id = request.resource_id.clone();
        let request_id = request.request_id.clone();
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "save_gb_resource_confirmation",
            &session.identity.node_id,
            &request_id,
            &resource_id,
        );
        let response = edge.response(client.save_gb_resource_confirmation(request).await)?;
        let Some(resource) = response.resource else {
            edge.invalid_response("empty_resource");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 resource".to_string(),
            ));
        };
        edge.success();
        Ok(resource)
    }

    pub async fn reset_gb_resource_confirmation(
        &self,
        request: ResetGbResourceConfirmationRequest,
    ) -> GuardResult<GbResource> {
        let device = self
            .get_gb_device(&request.device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {}", request.device_id)))?;
        let session = self.gb_session_for_device(&device, "reset_gb_resource_confirmation")?;
        let resource_id = request.resource_id.clone();
        let request_id = request.request_id.clone();
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "reset_gb_resource_confirmation",
            &session.identity.node_id,
            &request_id,
            &resource_id,
        );
        let response = edge.response(client.reset_gb_resource_confirmation(request).await)?;
        let Some(resource) = response.resource else {
            edge.invalid_response("empty_resource");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 resource".to_string(),
            ));
        };
        edge.success();
        Ok(resource)
    }

    pub async fn snapshot_image(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        count: u32,
        interval: u32,
    ) -> GuardResult<String> {
        let device = self
            .get_gb_device(device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
        let node_id = device.session_node_id;
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "snapshot_image", &node_id));
        }
        let mut client = self.session_client(&session).await?;
        let request = SnapshotImageRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            count,
            interval,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.snapshot_image, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "snapshot_image",
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let response = edge.response(client.snapshot_image(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "snapshot_image",
                error,
                "snapshot_rejected",
                "抓拍请求未被设备接受，请确认设备在线且支持抓拍",
                true,
            ));
        }
        if response.session_id.is_empty() {
            edge.invalid_response("empty_session_id");
            return Err(GuardError::Conflict(
                "session snapshot returned empty session id".to_string(),
            ));
        }
        edge.success();
        Ok(response.session_id)
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

    pub fn validate_live_start(&self) -> GuardResult<()> {
        self.select_any_session().map(|_| ())
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

    pub async fn start_broadcast(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_broadcast_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_broadcast_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Broadcast,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_broadcast_operation(
        &self,
        operation_id: &str,
        options: BroadcastOperationOptions,
    ) -> GuardResult<BroadcastOperationSummary> {
        let setup_guard = BROADCAST_OPERATION_SETUP.lock().await;
        if let Some(existing) = self.store.find_broadcast_operation_by_request(operation_id) {
            return Ok(broadcast_operation_summary(existing));
        }
        if options.targets.is_empty() {
            return Err(GuardError::InvalidConfig(
                "broadcast_targets_required".to_string(),
            ));
        }
        if options.targets.len() > 50 {
            return Err(GuardError::InvalidConfig(
                "broadcast_target_capacity_exceeded".to_string(),
            ));
        }

        let default_transport = normalize_broadcast_transport(&options.default_trans_mode)?;
        let mut resolved = Vec::with_capacity(options.targets.len());
        let mut target_keys = HashSet::with_capacity(options.targets.len());
        for mut target in options.targets {
            if target.device_id.trim().is_empty() || target.channel_id.trim().is_empty() {
                return Err(GuardError::InvalidConfig(
                    "broadcast_target_identity_required".to_string(),
                ));
            }
            if target.session_node_id.trim().is_empty() {
                target.session_node_id = self
                    .get_gb_device(&target.device_id)
                    .await?
                    .ok_or_else(|| {
                        GuardError::NotFound(format!("GB28181 device {}", target.device_id))
                    })?
                    .session_node_id;
            }
            let transport = if target.trans_mode.trim().is_empty() {
                default_transport.clone()
            } else {
                normalize_broadcast_transport(&target.trans_mode)?
            };
            let target_key = format!(
                "{}:{}:{}",
                target.device_id, target.channel_id, target.session_node_id
            );
            if !target_keys.insert(target_key.clone()) {
                return Err(GuardError::Conflict(
                    "duplicate_broadcast_target".to_string(),
                ));
            }
            target.trans_mode = transport;
            resolved.push((target_key, target));
        }

        let transports = resolved
            .iter()
            .map(|(_, target)| target.trans_mode.as_str())
            .collect::<Vec<_>>();
        let stream_node = self.select_broadcast_stream_node(&transports, resolved.len())?;
        let broadcast_id = format!("broadcast-{}", Uuid::now_v7());
        let shared_token = if options.token.trim().is_empty() {
            format!("gmv-{operation_id}")
        } else {
            options.token
        };
        let mut operation = BroadcastOperationRecord {
            broadcast_id: broadcast_id.clone(),
            operation_id: operation_id.to_string(),
            stream_node_id: stream_node.identity.node_id.clone(),
            input_url: String::new(),
            state: "starting".to_string(),
            targets: resolved
                .iter()
                .enumerate()
                .map(|(index, (target_key, target))| BroadcastTargetRecord {
                    target_key: target_key.clone(),
                    device_id: target.device_id.clone(),
                    channel_id: target.channel_id.clone(),
                    session_node_id: target.session_node_id.clone(),
                    leg_id: format!("{broadcast_id}-{:02}", index + 1),
                    transport: target.trans_mode.clone(),
                    profile: String::new(),
                    state: "starting".to_string(),
                    reason: String::new(),
                })
                .collect(),
        };
        self.store.upsert_broadcast_operation(operation.clone());
        drop(setup_guard);

        for index in 0..operation.targets.len() {
            let target = operation.targets[index].clone();
            let result = self
                .start_broadcast_with_options(
                    &format!("{operation_id}-leg-{}", index + 1),
                    &target.device_id,
                    &target.channel_id,
                    DeviceStreamOptions {
                        session_node_id: target.session_node_id.clone(),
                        token: shared_token.clone(),
                        trans_mode: target.transport.clone(),
                        broadcast_codec: options.codec.clone(),
                        broadcast_sample_rate: options.sample_rate,
                        broadcast_channel_count: options.channel_count,
                        broadcast_frame_duration_ms: options.frame_duration_ms,
                        broadcast_id: broadcast_id.clone(),
                        broadcast_leg_id: target.leg_id.clone(),
                        expected_stream_node_id: stream_node.identity.node_id.clone(),
                        ..DeviceStreamOptions::default()
                    },
                )
                .await;
            match result {
                Ok(summary)
                    if operation.input_url.is_empty()
                        || operation.input_url == summary.endpoint =>
                {
                    operation.input_url = summary.endpoint;
                    operation.targets[index].profile = summary.broadcast_profile;
                    operation.targets[index].state = "running".to_string();
                }
                Ok(summary) => {
                    let _ = self
                        .stop_stream(
                            &format!("{operation_id}-rollback-{}", index + 1),
                            &summary.stream_id,
                        )
                        .await;
                    operation.targets[index].state = "failed".to_string();
                    operation.targets[index].reason =
                        "broadcast_input_endpoint_mismatch".to_string();
                }
                Err(error) => {
                    base::log::warn!(
                        "broadcast target start failed: broadcast_id={}, leg_id={}, device_id={}, channel_id={}, reason={error}",
                        broadcast_id,
                        target.leg_id,
                        target.device_id,
                        target.channel_id
                    );
                    operation.targets[index].state = "failed".to_string();
                    operation.targets[index].reason = "broadcast_target_start_failed".to_string();
                }
            }
            self.store.upsert_broadcast_operation(operation.clone());
        }
        operation.state = aggregate_broadcast_start_state(&operation.targets).to_string();
        self.store.upsert_broadcast_operation(operation.clone());
        Ok(broadcast_operation_summary(operation))
    }

    pub fn get_broadcast_operation(
        &self,
        broadcast_id: &str,
    ) -> GuardResult<BroadcastOperationSummary> {
        self.store
            .get_broadcast_operation(broadcast_id)
            .map(broadcast_operation_summary)
            .ok_or_else(|| GuardError::NotFound(format!("broadcast {broadcast_id}")))
    }

    pub async fn stop_broadcast_target(
        &self,
        operation_id: &str,
        broadcast_id: &str,
        leg_id: &str,
    ) -> GuardResult<BroadcastOperationSummary> {
        let mut operation = self
            .store
            .get_broadcast_operation(broadcast_id)
            .ok_or_else(|| GuardError::NotFound(format!("broadcast {broadcast_id}")))?;
        let target = operation
            .targets
            .iter_mut()
            .find(|target| target.leg_id == leg_id)
            .ok_or_else(|| GuardError::NotFound(format!("broadcast leg {leg_id}")))?;
        if target.state == "running" || target.state == "starting" {
            match self.stop_stream(operation_id, leg_id).await {
                Ok(summary) => {
                    target.state = match summary.state {
                        StreamSummaryState::Stopped => "stopped",
                        _ => "stopping",
                    }
                    .to_string();
                    target.reason.clear();
                }
                Err(error) => {
                    base::log::warn!(
                        "broadcast target stop failed: broadcast_id={broadcast_id}, leg_id={leg_id}, reason={error}"
                    );
                    target.state = "failed".to_string();
                    target.reason = "broadcast_target_stop_failed".to_string();
                }
            }
        }
        operation.state = aggregate_broadcast_runtime_state(&operation.targets).to_string();
        self.store.upsert_broadcast_operation(operation.clone());
        Ok(broadcast_operation_summary(operation))
    }

    pub async fn stop_broadcast_operation(
        &self,
        operation_id: &str,
        broadcast_id: &str,
    ) -> GuardResult<BroadcastOperationSummary> {
        let mut operation = self
            .store
            .get_broadcast_operation(broadcast_id)
            .ok_or_else(|| GuardError::NotFound(format!("broadcast {broadcast_id}")))?;
        let active_legs = operation
            .targets
            .iter()
            .filter(|target| target.state == "running" || target.state == "starting")
            .map(|target| target.leg_id.clone())
            .collect::<Vec<_>>();
        let mut stops = base::tokio::task::JoinSet::new();
        for (index, leg_id) in active_legs.iter().enumerate() {
            let control = self.clone();
            let leg_id = leg_id.clone();
            let leg_operation_id = format!("{operation_id}-leg-{}", index + 1);
            stops.spawn(async move {
                let result = control.stop_stream(&leg_operation_id, &leg_id).await;
                (leg_id, result)
            });
        }
        while let Some(result) = stops.join_next().await {
            let (leg_id, stop_result) = result.map_err(|error| {
                GuardError::user_visible(
                    "broadcast_stop_task_failed",
                    format!("broadcast stop task failed: {error}"),
                    "广播停止任务异常，请重试",
                    true,
                    BTreeMap::new(),
                )
            })?;
            let target = operation
                .targets
                .iter_mut()
                .find(|target| target.leg_id == leg_id)
                .expect("active broadcast leg must belong to operation");
            match stop_result {
                Ok(summary) => {
                    target.state = match summary.state {
                        StreamSummaryState::Stopped => "stopped",
                        _ => "stopping",
                    }
                    .to_string();
                    target.reason.clear();
                }
                Err(error) => {
                    base::log::warn!(
                        "broadcast target stop failed: broadcast_id={broadcast_id}, leg_id={leg_id}, reason={error}"
                    );
                    target.state = "failed".to_string();
                    target.reason = "broadcast_target_stop_failed".to_string();
                }
            }
        }
        operation.state = aggregate_broadcast_runtime_state(&operation.targets).to_string();
        self.store.upsert_broadcast_operation(operation.clone());
        Ok(broadcast_operation_summary(operation))
    }

    async fn start_device_stream(
        &self,
        kind: DeviceStreamKind,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        let requested_session_node_id = options.session_node_id.trim().to_string();
        let select_session = || {
            if requested_session_node_id.is_empty() {
                self.select_node(NodeKind::Session, kind.session_capability())
            } else {
                self.select_session_node(
                    &requested_session_node_id,
                    kind.session_capability(),
                    kind.action(),
                )
            }
        };
        let stream_profile = normalize_stream_profile(kind, &options.stream_profile)?;
        let input_key = kind
            .input_key(device_id, channel_id, stream_profile)
            .map(|key| {
                if requested_session_node_id.is_empty() {
                    key
                } else {
                    format!("session:{requested_session_node_id}:{key}")
                }
            });
        let session = match input_key.as_deref() {
            Some(key) => match self.store.get_stream_session_owner_by_input(key) {
                Some(owner) => match self.session_node_for_owner(&owner) {
                    Ok(session) => session,
                    Err(_) if owner.stream_id.is_empty() => {
                        let candidate = select_session()?;
                        let owner = self.store.replace_inactive_stream_input_owner(
                            key,
                            StreamSessionOwnerRecord {
                                stream_id: String::new(),
                                input_key: key.to_string(),
                                node_id: candidate.identity.node_id,
                                instance_id: candidate.identity.instance_id,
                            },
                        );
                        self.session_node_for_owner(&owner)?
                    }
                    Err(error) => return Err(error),
                },
                None => {
                    let candidate = select_session()?;
                    let owner = self.store.claim_stream_input_owner(
                        key,
                        StreamSessionOwnerRecord {
                            stream_id: String::new(),
                            input_key: key.to_string(),
                            node_id: candidate.identity.node_id,
                            instance_id: candidate.identity.instance_id,
                        },
                    );
                    self.session_node_for_owner(&owner)?
                }
            },
            None => select_session()?,
        };
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
        let requested_subscription_id = token.clone();
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
            audio_codec: options.audio_codec,
            broadcast_codec: options.broadcast_codec,
            broadcast_sample_rate: options.broadcast_sample_rate,
            broadcast_channel_count: options.broadcast_channel_count,
            broadcast_frame_duration_ms: options.broadcast_frame_duration_ms,
            playback_id: options.playback_id.clone(),
            broadcast_id: options.broadcast_id,
            broadcast_leg_id: options.broadcast_leg_id,
            expected_stream_node_id: options.expected_stream_node_id,
            video_stream_profile: proto_stream_profile(stream_profile),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.start_{}, node={}, req: operation={:?}, device_id={}, channel_id={}, token={}, start_time_sec={}, end_time_sec={}, trans_mode={}, output_type={}, audio_codec={}, broadcast_codec={}, broadcast_sample_rate={}, broadcast_channel_count={}, broadcast_frame_duration_ms={}, expected_session={:?}",
            kind.prefix(),
            session.identity.node_id,
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
        let edge = RpcEdge::new(
            "session",
            kind.action(),
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let session_response = edge.response(match kind {
            DeviceStreamKind::Live => session_client.start_live(request).await,
            DeviceStreamKind::Playback => session_client.start_playback(request).await,
            DeviceStreamKind::Download => session_client.start_download(request).await,
            DeviceStreamKind::Broadcast => session_client.start_broadcast(request).await,
        })?;
        if let Some(error) = non_empty_error(session_response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                kind.action(),
                error,
                "stream_start_failed",
                "视频流创建失败，请检查设备在线状态和媒体服务",
                true,
            ));
        }
        if session_response.state != DeviceStreamState::Running as i32 {
            edge.invalid_response("stream_not_running");
            return Err(GuardError::Conflict(format!(
                "session did not enter {} running state",
                kind.prefix()
            )));
        }
        edge.success();
        let stream_id = session_response.stream_id.clone();
        self.store
            .upsert_stream_session_owner(StreamSessionOwnerRecord {
                stream_id: stream_id.clone(),
                input_key: input_key.unwrap_or_default(),
                node_id: session.identity.node_id.clone(),
                instance_id: session.identity.instance_id.clone(),
            });
        let allocation = self
            .store
            .resolve_active_allocation(&session_response.stream_id)?;
        let (lease, route) = allocation
            .as_ref()
            .map(|(lease, route)| (Some(lease), Some(route)))
            .unwrap_or((None, None));
        Ok(StreamSummary {
            stream_id,
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
            lease_id: lease
                .map(|lease| lease.lease_id.clone())
                .unwrap_or_default(),
            route_id: route
                .map(|route| route.route_id.clone())
                .unwrap_or_default(),
            endpoint: session_response.endpoint,
            video_codec: session_response.video_codec,
            audio_codec: session_response.audio_codec,
            broadcast_profile: session_response.broadcast_profile,
            requested_stream_profile: matches!(kind, DeviceStreamKind::Live)
                .then(|| stream_profile_name(session_response.requested_stream_profile))
                .unwrap_or_default(),
            effective_stream_profile: matches!(kind, DeviceStreamKind::Live)
                .then(|| stream_profile_name(session_response.effective_stream_profile))
                .unwrap_or_default(),
            stream_profile_verification: matches!(kind, DeviceStreamKind::Live)
                .then(|| {
                    stream_profile_verification_name(session_response.stream_profile_verification)
                })
                .unwrap_or_default(),
            subscription_id: if session_response.subscription_id.is_empty() {
                requested_subscription_id
            } else {
                session_response.subscription_id
            },
            session_node_id: session.identity.node_id.clone(),
            session_instance_id: session.identity.instance_id.clone(),
            playback_id: session_response.playback_id,
            playback_generation: session_response.playback_generation,
            playback_start_time_sec: options.start_time_sec,
            playback_end_time_sec: options.end_time_sec,
            state: StreamSummaryState::Running,
        })
    }

    pub async fn stop_stream(
        &self,
        operation_id: &str,
        stream_id: &str,
    ) -> GuardResult<StreamSummary> {
        let active_route_id = match self.store.resolve_active_allocation(stream_id) {
            Ok(Some((_, route))) => Some(route.route_id),
            Ok(None) => None,
            Err(error) => {
                base::log::warn!(
                    "guard stream stop found inconsistent allocation projection: stream_id={}, reason={}",
                    stream_id,
                    error
                );
                None
            }
        };
        let session = self.session_for_stream(stream_id)?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let request = StopDeviceStreamRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            stream_id: stream_id.to_string(),
            reason: "manual_stop".to_string(),
            subscription_id: String::new(),
            force: true,
            expected_session: Some(proto_identity(&session.identity)),
            stop_reason: "Guard 强制停止".to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.stop_device_stream, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "stop_device_stream",
            &session.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(session_client.stop_device_stream(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "stop_device_stream",
                error,
                "stream_stop_failed",
                "停止视频流失败，请稍后重试",
                true,
            ));
        }
        let state = DeviceStreamState::try_from(response.state).map_err(|_| {
            edge.invalid_response("stream_stop_invalid_state");
            GuardError::Conflict("session returned invalid stream stop state".to_string())
        })?;
        if !matches!(
            state,
            DeviceStreamState::Stopping | DeviceStreamState::Stopped
        ) {
            edge.invalid_response("stream_not_stopped");
            return Err(GuardError::Conflict(
                "session did not accept device stream stop".to_string(),
            ));
        }
        edge.success();
        let session_node_id = session.identity.node_id.clone();
        let session_instance_id = session.identity.instance_id.clone();
        if state == DeviceStreamState::Stopped {
            self.store.remove_stream_session_owner(stream_id);
            if let Some(mut route) = active_route_id
                .as_deref()
                .and_then(|route_id| self.store.get_route(route_id))
            {
                route.state = RouteState::Closed;
                self.store.upsert_route(route);
            }
        }
        Ok(StreamSummary {
            stream_id: stream_id.to_string(),
            device_id: String::new(),
            channel_id: String::new(),
            node_id: session_node_id.clone(),
            instance_id: session_instance_id.clone(),
            lease_id: String::new(),
            route_id: String::new(),
            endpoint: String::new(),
            video_codec: String::new(),
            audio_codec: String::new(),
            broadcast_profile: String::new(),
            requested_stream_profile: String::new(),
            effective_stream_profile: String::new(),
            stream_profile_verification: String::new(),
            subscription_id: String::new(),
            session_node_id,
            session_instance_id,
            playback_id: String::new(),
            playback_generation: 0,
            playback_start_time_sec: 0,
            playback_end_time_sec: 0,
            state: if state == DeviceStreamState::Stopped {
                StreamSummaryState::Stopped
            } else {
                StreamSummaryState::Stopping
            },
        })
    }

    pub async fn release_stream(
        &self,
        operation_id: &str,
        stream_id: &str,
        subscription_id: &str,
    ) -> GuardResult<StreamSummary> {
        if subscription_id.trim().is_empty() {
            return Err(GuardError::InvalidConfig(
                "subscription_id is required".to_string(),
            ));
        }
        let active_route_id = match self.store.resolve_active_allocation(stream_id) {
            Ok(Some((_, route))) => Some(route.route_id),
            Ok(None) => None,
            Err(error) => {
                base::log::warn!(
                    "guard stream release found inconsistent allocation projection: stream_id={}, reason={}",
                    stream_id,
                    error
                );
                None
            }
        };
        let session = self.session_for_stream(stream_id)?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let request = StopDeviceStreamRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            reason: "viewer_release".to_string(),
            subscription_id: subscription_id.to_string(),
            force: false,
            expected_session: None,
            stop_reason: String::new(),
        };
        let edge = RpcEdge::new(
            "session",
            "release_device_stream",
            &session.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(session_client.stop_device_stream(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "release_device_stream",
                error,
                "stream_release_failed",
                "释放视频订阅失败，请稍后重试",
                true,
            ));
        }
        let state = if response.state == DeviceStreamState::Running as i32 {
            StreamSummaryState::Running
        } else if response.state == DeviceStreamState::Stopped as i32 {
            self.store.remove_stream_session_owner(stream_id);
            if let Some(mut route) = active_route_id
                .as_deref()
                .and_then(|route_id| self.store.get_route(route_id))
            {
                route.state = RouteState::Closed;
                self.store.upsert_route(route);
            }
            StreamSummaryState::Stopped
        } else {
            edge.invalid_response("stream_release_invalid_state");
            return Err(GuardError::Conflict(
                "session returned invalid release state".to_string(),
            ));
        };
        edge.success();
        Ok(StreamSummary {
            stream_id: stream_id.to_string(),
            device_id: String::new(),
            channel_id: String::new(),
            node_id: String::new(),
            instance_id: String::new(),
            lease_id: String::new(),
            route_id: String::new(),
            endpoint: String::new(),
            video_codec: String::new(),
            audio_codec: String::new(),
            broadcast_profile: String::new(),
            requested_stream_profile: String::new(),
            effective_stream_profile: String::new(),
            stream_profile_verification: String::new(),
            subscription_id: subscription_id.to_string(),
            session_node_id: session.identity.node_id,
            session_instance_id: session.identity.instance_id,
            playback_id: String::new(),
            playback_generation: 0,
            playback_start_time_sec: 0,
            playback_end_time_sec: 0,
            state,
        })
    }

    pub async fn create_stream_output(
        &self,
        operation_id: &str,
        stream_id: &str,
        output_type: &str,
        audio_codec: &str,
        subscription_id: &str,
    ) -> GuardResult<StreamOutputSummary> {
        let stream = self.stream_node_for_resource(stream_id)?;
        let stream_grpc = grpc_uri(&stream)?;
        let mut client = StreamControlClient::new(connect_rpc(&stream_grpc, "stream").await?);
        let request = CreateOutputRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            output_type: output_type.to_string(),
            endpoint_mode: EndpointMode::Single as i32,
            audio_codec: audio_codec.to_string(),
            subscription_id: subscription_id.to_string(),
        };
        let edge = RpcEdge::new(
            "stream",
            "create_output",
            &stream.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(client.create_output(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "stream",
                "create_output",
                error,
                "stream_output_create_failed",
                "媒体输出创建失败",
                true,
            ));
        }
        let output = response.output.ok_or_else(|| {
            edge.invalid_response("output_missing");
            GuardError::Conflict("stream create_output returned no output".to_string())
        })?;
        edge.success();
        Ok(stream_output_summary(output))
    }

    pub async fn list_stream_outputs(
        &self,
        stream_id: &str,
    ) -> GuardResult<Vec<StreamOutputSummary>> {
        let stream = self.stream_node_for_resource(stream_id)?;
        let stream_grpc = grpc_uri(&stream)?;
        let mut client = StreamControlClient::new(connect_rpc(&stream_grpc, "stream").await?);
        let edge = RpcEdge::new(
            "stream",
            "get_playback_endpoints",
            &stream.identity.node_id,
            "list-stream-outputs",
            stream_id,
        );
        let response = edge.response(
            client
                .get_playback_endpoints(GetPlaybackEndpointsRequest {
                    stream_id: stream_id.to_string(),
                })
                .await,
        )?;
        edge.success();
        Ok(response
            .outputs
            .into_iter()
            .map(stream_output_summary)
            .collect())
    }

    pub fn validate_stream_output_target(
        &self,
        stream_id: &str,
        output_type: &str,
    ) -> GuardResult<()> {
        self.stream_node_for_resource(stream_id)?;
        if output_type.trim().eq_ignore_ascii_case("ll_hls")
            && self
                .store
                .has_playback_ticket_for_stream(stream_id, now_ms())
        {
            return Err(user_error(
                "OUTPUT_NOT_ALLOWED_FOR_PLAYBACK",
                "ll_hls output is only allowed for live preview",
                "LL-HLS 仅支持直播，请在回放中使用普通 HLS",
                false,
                BTreeMap::new(),
            ));
        }
        Ok(())
    }

    pub async fn close_stream_output(
        &self,
        operation_id: &str,
        stream_id: &str,
        output_id: &str,
    ) -> GuardResult<bool> {
        let stream = self.stream_node_for_resource(stream_id)?;
        let stream_grpc = grpc_uri(&stream)?;
        let mut client = StreamControlClient::new(connect_rpc(&stream_grpc, "stream").await?);
        let request = CloseOutputRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            output_id: output_id.to_string(),
            stream_id: stream_id.to_string(),
        };
        let edge = RpcEdge::new(
            "stream",
            "close_output",
            &stream.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(client.close_output(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "stream",
                "close_output",
                error,
                "stream_output_close_failed",
                "媒体输出关闭失败",
                true,
            ));
        }
        edge.success();
        Ok(response.closed)
    }

    fn stream_node_for_resource(&self, stream_id: &str) -> GuardResult<NodeRecord> {
        let (_, route) = self
            .store
            .resolve_active_allocation(stream_id)?
            .ok_or_else(|| GuardError::NotFound(format!("stream {stream_id}")))?;
        self.store
            .get_node(&route.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", route.node_id)))
    }

    pub async fn set_playback_speed(
        &self,
        operation_id: &str,
        stream_id: &str,
        speed_rate: f32,
    ) -> GuardResult<()> {
        self.set_playback_speed_versioned(operation_id, "", stream_id, speed_rate, 0)
            .await
            .map(|_| ())
    }

    pub async fn set_playback_speed_versioned(
        &self,
        operation_id: &str,
        playback_id: &str,
        stream_id: &str,
        speed_rate: f32,
        expected_generation: u64,
    ) -> GuardResult<u64> {
        if !matches!(speed_rate, 0.5 | 1.0 | 2.0 | 4.0) {
            return Err(GuardError::user_visible(
                "invalid_playback_speed",
                "unsupported playback speed",
                "only 0.5x, 1x, 2x and 4x are supported",
                false,
                BTreeMap::new(),
            ));
        }
        let session = self.session_for_stream(stream_id)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&grpc_uri(&session)?, "session").await?);
        let request = SetPlaybackSpeedRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            speed_rate,
            playback_id: playback_id.to_string(),
            expected_generation,
        };
        let edge = RpcEdge::new(
            "session",
            "set_playback_speed",
            &session.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(session_client.set_playback_speed(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "set_playback_speed",
                error,
                "playback_speed_failed",
                "playback speed change failed",
                false,
            ));
        }
        if !response.accepted {
            edge.invalid_response("playback_speed_not_accepted");
            return Err(GuardError::Conflict(
                "playback speed was not accepted".to_string(),
            ));
        }
        edge.success();
        Ok(response.generation)
    }

    pub async fn seek_playback(
        &self,
        operation_id: &str,
        playback_id: &str,
        stream_id: &str,
        position_sec: u32,
        expected_generation: u64,
    ) -> GuardResult<u64> {
        let session = self.session_for_stream(stream_id)?;
        let mut client =
            SessionControlClient::new(connect_rpc(&grpc_uri(&session)?, "session").await?);
        let response = client
            .seek_playback(SeekPlaybackRequest {
                operation: Some(OperationRef {
                    operation_id: operation_id.to_string(),
                    idempotency_key: operation_id.to_string(),
                }),
                playback_id: playback_id.to_string(),
                stream_id: stream_id.to_string(),
                position_sec,
                expected_generation,
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("playback seek RPC failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            return Err(remote_error(
                "session",
                "seek_playback",
                error,
                "playback_seek_failed",
                "playback seek failed",
                true,
            ));
        }
        if !response.accepted {
            return Err(GuardError::Conflict(
                "playback seek was not accepted".to_string(),
            ));
        }
        Ok(response.generation)
    }

    pub async fn set_playback_state(
        &self,
        operation_id: &str,
        playback_id: &str,
        stream_id: &str,
        paused: bool,
        expected_generation: u64,
        subscription_id: &str,
    ) -> GuardResult<u64> {
        let session = self.session_for_stream(stream_id)?;
        let mut client =
            SessionControlClient::new(connect_rpc(&grpc_uri(&session)?, "session").await?);
        let response = client
            .set_playback_state(SetPlaybackStateRequest {
                operation: Some(OperationRef {
                    operation_id: operation_id.to_string(),
                    idempotency_key: operation_id.to_string(),
                }),
                playback_id: playback_id.to_string(),
                stream_id: stream_id.to_string(),
                state: if paused {
                    PlaybackState::Paused as i32
                } else {
                    PlaybackState::Playing as i32
                },
                expected_generation,
                subscription_id: subscription_id.to_string(),
            })
            .await
            .map_err(|error| GuardError::Conflict(format!("playback state RPC failed: {error}")))?
            .into_inner();
        if let Some(error) = non_empty_error(response.error) {
            return Err(remote_error(
                "session",
                "set_playback_state",
                error,
                "playback_state_failed",
                "playback state change failed",
                true,
            ));
        }
        if !response.accepted {
            return Err(GuardError::Conflict(
                "playback state was not accepted".to_string(),
            ));
        }
        Ok(response.generation)
    }

    pub async fn refresh_playback_presences(
        &self,
        items: Vec<PlaybackPresenceHeartbeat>,
    ) -> GuardResult<(i64, Vec<PlaybackPresenceHeartbeatResult>)> {
        let mut groups = HashMap::<String, (NodeRecord, Vec<PlaybackPresenceHeartbeat>)>::new();
        for item in items {
            let session = self.session_for_stream(&item.stream_id)?;
            groups
                .entry(session.identity.node_id.clone())
                .or_insert_with(|| (session, Vec::new()))
                .1
                .push(item);
        }
        let mut server_time_ms = 0;
        let mut results = Vec::new();
        for (_, (session, items)) in groups {
            let mut client =
                SessionControlClient::new(connect_rpc(&grpc_uri(&session)?, "session").await?);
            let response = client
                .refresh_playback_presence(RefreshPlaybackPresenceRequest { items })
                .await
                .map_err(|error| {
                    GuardError::Conflict(format!("playback presence heartbeat RPC failed: {error}"))
                })?
                .into_inner();
            server_time_ms = server_time_ms.max(response.server_time_ms);
            results.extend(response.items);
        }
        Ok((server_time_ms, results))
    }

    pub async fn ptz(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        command: &str,
        speed: u32,
    ) -> GuardResult<u64> {
        let session = self.select_node(NodeKind::Session, "device.ptz")?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let request = ControlPtzRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            command: command.to_string(),
            speed,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.control_ptz, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "control_ptz",
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let response = edge.response(session_client.control_ptz(request).await)?;
        if !response.accepted {
            if let Some(error) = response.error.as_ref() {
                edge.business_rejection(error);
            } else {
                edge.invalid_response("ptz_not_accepted_without_error");
            }
            return Err(response
                .error
                .filter(|error| !error.code.is_empty() || !error.message.is_empty())
                .map(|error| {
                    remote_error(
                        "session",
                        "control_ptz",
                        error,
                        "ptz_rejected",
                        "云台控制未被设备接受，请确认通道支持云台",
                        false,
                    )
                })
                .unwrap_or_else(|| {
                    user_error(
                        "ptz_rejected",
                        "session ptz rejected",
                        "云台控制未被设备接受，请确认通道支持云台",
                        false,
                        detail_pairs([("service", "session"), ("action", "control_ptz")]),
                    )
                }));
        }
        edge.success();
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
                resource_id: task_id.clone(),
                capability: capability.clone(),
                zone: avai.zone.clone(),
                constraints: std::collections::HashMap::new(),
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
            stream_type: capability.clone(),
            idempotency_key: format!("ai-{operation_id}"),
            owner: avai.identity.clone(),
            constraints: std::collections::HashMap::new(),
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
        let request = CreateTaskRequest {
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
        };
        base::log::debug!(
            "guard rpc client outbound: method=avai_control.create_task, node={}, req: operation={:?}, task_id={}, task_type={}, route_id={}, expected_avai={:?}, payload_bytes={}",
            avai.identity.node_id,
            request.operation,
            request.task_id,
            request.task_type,
            request.route_id,
            request.expected_avai,
            request.payload.len()
        );
        let edge = RpcEdge::new(
            "avai",
            "create_task",
            &avai.identity.node_id,
            operation_id,
            &task_id,
        );
        let response = edge.response(avai_client.create_task(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            let _ =
                LeaseService::new(self.store.clone()).fail(&lease_id, &avai.identity.instance_id);
            return Err(remote_error(
                "avai",
                "create_task",
                error,
                "avai_task_rejected",
                "AI 任务创建失败，请检查目标节点状态后重试",
                true,
            ));
        }
        if response.state != AiTaskState::Running as i32 {
            edge.invalid_response("task_not_running");
            return Err(GuardError::Conflict(
                "avai task did not enter running state".to_string(),
            ));
        }
        edge.success();
        LeaseService::new(self.store.clone()).confirm(&lease_id, &avai.identity.instance_id)?;
        RouteService::new(self.store.clone()).apply_snapshot(ResourceSnapshot {
            owner: avai.identity.clone(),
            generation: 1,
            sequence: 1,
            full: false,
            resources: vec![SnapshotResource {
                resource_id: task_id.clone(),
                resource_type: "ai_task".to_string(),
                route_id: Some(route_id.clone()),
                lease_id: Some(lease_id.clone()),
                route_state: RouteState::Running,
                endpoints: Vec::new(),
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

    pub async fn cancel_ai(&self, operation_id: &str, task_id: &str) -> GuardResult<AiTaskSummary> {
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
        let request = CancelTaskRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            task_id: task_id.to_string(),
            reason: "manual".to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=avai_control.cancel_task, node={}, req:{request:?}",
            avai.identity.node_id
        );
        let edge = RpcEdge::new(
            "avai",
            "cancel_task",
            &avai.identity.node_id,
            operation_id,
            task_id,
        );
        let response = edge.response(avai_client.cancel_task(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "avai",
                "cancel_task",
                error,
                "avai_cancel_rejected",
                "AI 任务取消失败，请检查目标节点状态后重试",
                true,
            ));
        }
        if response.state != AiTaskState::Cancelled as i32 {
            edge.invalid_response("task_not_cancelled");
            return Err(GuardError::Conflict(
                "avai task did not enter cancelled state".to_string(),
            ));
        }
        edge.success();
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
        self.session_nodes()
            .into_iter()
            .next()
            .ok_or_else(|| GuardError::NotFound("no connected session node".to_string()))
    }

    fn session_for_stream(&self, stream_id: &str) -> GuardResult<NodeRecord> {
        if let Some(owner) = self.store.get_stream_session_owner(stream_id) {
            return self.session_node_for_owner(&owner);
        }
        let sessions = self.session_nodes();
        if sessions.len() == 1 {
            return Ok(sessions.into_iter().next().expect("one session node"));
        }
        Err(GuardError::Conflict(format!(
            "session owner for stream {stream_id} is unknown"
        )))
    }

    fn session_node_for_owner(&self, owner: &StreamSessionOwnerRecord) -> GuardResult<NodeRecord> {
        let node = self
            .store
            .get_node(&owner.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", owner.node_id)))?;
        if node.identity.instance_id != owner.instance_id
            || node.connection != ConnectionState::Connected
            || (owner.stream_id.is_empty() && node.scheduling != SchedulingState::Enabled)
        {
            return Err(GuardError::Conflict(format!(
                "session owner for stream {} is unavailable or stale",
                owner.stream_id
            )));
        }
        Ok(node)
    }

    fn session_nodes(&self) -> Vec<NodeRecord> {
        let mut nodes = self
            .store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == NodeKind::Session
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id));
        nodes
    }

    async fn session_client(
        &self,
        session: &NodeRecord,
    ) -> GuardResult<SessionControlClient<tonic::transport::Channel>> {
        let session_grpc = grpc_uri(session)?;
        Ok(SessionControlClient::new(
            connect_rpc(&session_grpc, "session").await?,
        ))
    }

    fn monitor_session_node(&self, session_node_id: &str) -> GuardResult<NodeRecord> {
        if session_node_id.trim().is_empty() {
            return Err(GuardError::InvalidConfig(
                "session_node_id is required".to_string(),
            ));
        }
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected {
            return Err(node_unavailable(
                "session",
                "stream_monitor",
                session_node_id,
            ));
        }
        Ok(session)
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

    fn select_broadcast_stream_node(
        &self,
        transports: &[&str],
        target_count: usize,
    ) -> GuardResult<NodeRecord> {
        let tcp_passive_count = transports
            .iter()
            .filter(|transport| **transport == "tcp_passive")
            .count();
        let operations = self.store.broadcast_operations();
        self.store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == NodeKind::Stream
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
                    && node.capabilities.iter().any(|item| item == "broadcast")
            })
            .filter(|node| {
                let rtp_endpoints = node
                    .endpoints
                    .iter()
                    .filter(|endpoint| endpoint.name == "rtp" || endpoint.scheme == "rtp")
                    .collect::<Vec<_>>();
                let supports = |requested: &str| {
                    requested == "udp"
                        || rtp_endpoints
                            .iter()
                            .filter_map(|endpoint| endpoint.labels.get("media_transports"))
                            .flat_map(|value| value.split(','))
                            .any(|value| value.trim() == requested)
                };
                let label_values = |name: &str| {
                    rtp_endpoints
                        .iter()
                        .filter_map(|endpoint| endpoint.labels.get(name))
                        .flat_map(|value| value.split(','))
                        .map(str::trim)
                        .collect::<HashSet<_>>()
                };
                let packetizations = label_values("broadcast_packetizations");
                let max_parents = rtp_endpoints
                    .iter()
                    .filter_map(|endpoint| endpoint.labels.get("max_broadcast_parents"))
                    .filter_map(|value| value.parse::<usize>().ok())
                    .max()
                    .unwrap_or(0);
                let max_legs = rtp_endpoints
                    .iter()
                    .filter_map(|endpoint| endpoint.labels.get("max_broadcast_legs"))
                    .filter_map(|value| value.parse::<usize>().ok())
                    .max()
                    .unwrap_or(0);
                let active_operations = operations
                    .iter()
                    .filter(|operation| {
                        operation.stream_node_id == node.identity.node_id
                            && operation.state != "stopped"
                            && operation.state != "failed"
                    })
                    .collect::<Vec<_>>();
                let active_legs = active_operations
                    .iter()
                    .flat_map(|operation| operation.targets.iter())
                    .filter(|target| {
                        target.state == "starting"
                            || target.state == "running"
                            || target.state == "stopping"
                    })
                    .count();
                transports.iter().all(|transport| supports(transport))
                    && packetizations.contains("raw_g711")
                    && packetizations.contains("rtp_ps_g711")
                    && active_operations.len() < max_parents
                    && target_count <= max_legs
                    && active_legs.saturating_add(target_count) <= max_legs
                    && (tcp_passive_count <= 1
                        || rtp_endpoints
                            .iter()
                            .any(|endpoint| endpoint.mode == EndpointModeRecord::Multi))
            })
            .min_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id))
            .ok_or_else(|| {
                GuardError::Capacity(
                    "no stream node has the required broadcast capability and capacity".to_string(),
                )
            })
    }

    fn select_session_node(
        &self,
        node_id: &str,
        capability: &str,
        action: &str,
    ) -> GuardResult<NodeRecord> {
        let node = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&node) || !node.capabilities.iter().any(|item| item == capability) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id} for {capability}"
            )));
        }
        if node.connection != ConnectionState::Connected
            || node.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", action, node_id));
        }
        Ok(node)
    }
}

fn normalize_broadcast_transport(value: &str) -> GuardResult<String> {
    let normalized = if value.trim().is_empty() {
        "udp"
    } else {
        value.trim()
    };
    match normalized {
        "udp" | "tcp_active" | "tcp_passive" => Ok(normalized.to_string()),
        _ => Err(GuardError::InvalidConfig(
            "invalid_media_transport".to_string(),
        )),
    }
}

fn aggregate_broadcast_start_state(targets: &[BroadcastTargetRecord]) -> &'static str {
    let running = targets
        .iter()
        .filter(|target| target.state == "running")
        .count();
    if running == targets.len() {
        "running"
    } else if running > 0 {
        "partial"
    } else {
        "failed"
    }
}

fn aggregate_broadcast_runtime_state(targets: &[BroadcastTargetRecord]) -> &'static str {
    if targets
        .iter()
        .any(|target| target.state == "running" || target.state == "starting")
    {
        "partial"
    } else if targets.iter().any(|target| target.state == "stopping") {
        "stopping"
    } else {
        "stopped"
    }
}

fn broadcast_operation_summary(operation: BroadcastOperationRecord) -> BroadcastOperationSummary {
    BroadcastOperationSummary {
        broadcast_id: operation.broadcast_id,
        stream_node_id: operation.stream_node_id,
        input_url: operation.input_url,
        state: operation.state,
        target_summaries: operation
            .targets
            .into_iter()
            .map(|target| BroadcastTargetSummary {
                target_key: target.target_key,
                device_id: target.device_id,
                channel_id: target.channel_id,
                session_node_id: target.session_node_id,
                leg_id: target.leg_id,
                transport: target.transport,
                profile: target.profile,
                state: target.state,
                reason: target.reason,
            })
            .collect(),
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
    let started = Instant::now();
    base::log::debug!("guard rpc client outbound: service={name}, endpoint={uri}");
    let channel = base_rpc::connect_channel(&config).await.map_err(|error| {
        base::log::debug!(
            "guard rpc client inbound: service={name}, endpoint={uri}, status=error, elapsed_ms={}, err={error}",
            started.elapsed().as_millis()
        );
        node_connect_error(name, error.to_string())
    })?;
    base::log::debug!(
        "guard rpc client inbound: service={name}, endpoint={uri}, status=ok, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(channel)
}

fn is_gb_session_node(node: &NodeRecord) -> bool {
    node.identity.kind == NodeKind::Session
        && (node.config.get("service").map(String::as_str) == Some("session-gb28181")
            || node.config.get("protocol").map(String::as_str) == Some("gb28181")
            || node
                .capabilities
                .iter()
                .any(|item| item == "protocol.gb28181"))
}
fn grpc_uri(node: &NodeRecord) -> GuardResult<String> {
    let endpoint = node
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| {
            user_error(
                "node_endpoint_missing",
                format!("node {} grpc endpoint missing", node.identity.node_id),
                "节点未上报 RPC 地址，请检查节点配置",
                false,
                detail_pairs([
                    ("node_id", node.identity.node_id.as_str()),
                    ("service", node_kind_name(node.identity.kind)),
                ]),
            )
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

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Session => "session",
        NodeKind::Stream => "stream",
        NodeKind::Avai => "avai",
    }
}

fn node_unavailable(service: &str, action: &str, node_id: &str) -> GuardError {
    user_error(
        "node_unavailable",
        format!("{service} node {node_id} is offline or disabled"),
        "节点离线或不可调度，请等待恢复或切换节点",
        true,
        detail_pairs([
            ("node_id", node_id),
            ("service", service),
            ("action", action),
        ]),
    )
}

fn node_connect_error(service: &str, message: String) -> GuardError {
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("deadline")
        || lower.contains("timeout")
        || lower.contains("timed out")
    {
        "node_rpc_timeout"
    } else if lower.contains("tls") || lower.contains("certificate") || lower.contains("handshake")
    {
        "node_rpc_tls_failed"
    } else {
        "node_rpc_connect_failed"
    };
    let user_message = match code {
        "node_rpc_timeout" => "节点响应超时，请稍后重试或检查节点负载/网络",
        "node_rpc_tls_failed" => "节点安全连接失败，请检查证书、域名或主机时间",
        _ => "无法连接目标节点，请检查节点进程、地址和网络",
    };
    user_error(
        code,
        format!("connect {service} RPC failed: {message}"),
        user_message,
        true,
        detail_pairs([("service", service)]),
    )
}

fn node_rpc_status(service: &str, action: &str, error: tonic::Status) -> GuardError {
    if let Some((global_code, output)) =
        gmv_nodec::error::global_error_output_from_tonic_status(&error)
    {
        let code = GmvGuardErrorCode::from_code(global_code)
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string());
        let mut details = detail_pairs([("service", service), ("action", action)]);
        details.insert("global_code".to_string(), global_code.to_string());
        details.insert("global_code_name".to_string(), output.code_name.to_string());
        details.insert("retryable".to_string(), output.retryable.to_string());
        return user_error(
            code,
            format!("{service} RPC {action} failed: {error}"),
            output.user_message,
            output.retryable,
            details,
        );
    }
    let code = match error.code() {
        tonic::Code::DeadlineExceeded => "node_rpc_timeout",
        tonic::Code::Unavailable => "node_rpc_unavailable",
        tonic::Code::Unauthenticated => "unauthorized",
        tonic::Code::PermissionDenied => "forbidden",
        tonic::Code::InvalidArgument => "bad_request",
        _ => "node_rpc_unavailable",
    };
    let user_message = match code {
        "node_rpc_timeout" => "节点响应超时，请稍后重试或检查节点负载/网络",
        "node_rpc_unavailable" => "无法连接目标节点，请检查节点进程、地址和网络",
        "unauthorized" => "节点认证失败，请检查服务间认证配置",
        "forbidden" => "目标节点拒绝执行此操作，请检查节点权限配置",
        "bad_request" => "请求参数不完整或不符合目标节点要求",
        _ => "节点调用失败，请稍后重试",
    };
    user_error(
        code,
        format!("{service} RPC {action} failed: {error}"),
        user_message,
        matches!(
            error.code(),
            tonic::Code::DeadlineExceeded | tonic::Code::Unavailable
        ),
        detail_pairs([("service", service), ("action", action)]),
    )
}

fn remote_error(
    service: &str,
    action: &str,
    error: ErrorDetail,
    fallback_code: &str,
    fallback_user_message: &str,
    retryable: bool,
) -> GuardError {
    let remote_code = error.code;
    let remote_message = error.message;
    let metadata = error.metadata;
    let registered_output = metadata
        .get(META_GLOBAL_CODE)
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|code| base::err::error_output(code).map(|output| (code, output)));
    let code = if let Some((global_code, output)) = &registered_output {
        GmvGuardErrorCode::from_code(*global_code)
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string())
    } else if remote_code.trim().is_empty() {
        fallback_code.to_string()
    } else {
        remote_code.clone()
    };
    let user_message = if let Some((_, output)) = &registered_output {
        output.user_message.to_string()
    } else if remote_message.trim().is_empty() {
        fallback_user_message.to_string()
    } else {
        remote_user_message(&code, &remote_message, fallback_user_message)
    };
    let retryable = registered_output
        .as_ref()
        .map_or(retryable, |(_, output)| output.retryable);
    let mut details = detail_pairs([("service", service), ("action", action)]);
    if !remote_code.trim().is_empty() {
        details.insert("remote_code".to_string(), remote_code.clone());
    }
    details.extend(metadata);
    let message = if remote_message.trim().is_empty() {
        format!("{service} RPC {action} rejected: {code}")
    } else {
        format!("{service} RPC {action} rejected: {remote_message}")
    };
    user_error(code, message, user_message, retryable, details)
}

fn remote_user_message(code: &str, message: &str, fallback: &str) -> String {
    if let Some(error_code) = GmvGuardErrorCode::from_api_code(code) {
        return error_code.out_msg().to_string();
    }
    if message.trim().is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback}：{message}")
    }
}

fn stream_output_summary(output: OutputInfo) -> StreamOutputSummary {
    let state = match OutputState::try_from(output.state).unwrap_or(OutputState::Failed) {
        OutputState::Preparing => StreamOutputState::Preparing,
        OutputState::Ready => StreamOutputState::Ready,
        OutputState::Closed => StreamOutputState::Closed,
        OutputState::Failed | OutputState::Unspecified => StreamOutputState::Failed,
    };
    StreamOutputSummary {
        output_id: output.output_id,
        stream_id: output.stream_id,
        output_type: output.output_type,
        endpoint: output.endpoint,
        state,
    }
}

fn user_error(
    code: impl Into<String>,
    message: impl Into<String>,
    user_message: impl Into<String>,
    retryable: bool,
    details: BTreeMap<String, String>,
) -> GuardError {
    GuardError::user_visible(code, message, user_message, retryable, details)
}

fn detail_pairs<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn normalize_stream_profile(kind: DeviceStreamKind, value: &str) -> GuardResult<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "main" => Ok("main"),
        "sub" if matches!(kind, DeviceStreamKind::Live) => Ok("sub"),
        "sub" => Err(GuardError::user_visible(
            "stream_profile_unsupported",
            "stream_profile is only supported for live preview",
            "主辅码流选择仅适用于实时点播",
            false,
            BTreeMap::new(),
        )),
        _ => Err(GuardError::user_visible(
            "invalid_stream_profile",
            "stream_profile must be main or sub",
            "码流类型必须是主码流或辅码流",
            false,
            BTreeMap::new(),
        )),
    }
}

fn proto_stream_profile(profile: &str) -> i32 {
    match profile {
        "sub" => VideoStreamProfile::Sub as i32,
        _ => VideoStreamProfile::Main as i32,
    }
}

fn stream_profile_name(value: i32) -> String {
    match VideoStreamProfile::try_from(value).unwrap_or(VideoStreamProfile::Unspecified) {
        VideoStreamProfile::Sub => "sub",
        VideoStreamProfile::Main | VideoStreamProfile::Unspecified => "main",
    }
    .to_string()
}

fn stream_profile_verification_name(value: i32) -> String {
    match StreamProfileVerification::try_from(value)
        .unwrap_or(StreamProfileVerification::Unspecified)
    {
        StreamProfileVerification::Confirmed => "confirmed",
        StreamProfileVerification::Unverified => "unverified",
        StreamProfileVerification::Unspecified => "unspecified",
    }
    .to_string()
}

#[derive(Debug, Clone, Copy)]
enum DeviceStreamKind {
    Live,
    Playback,
    Download,
    Broadcast,
}

impl DeviceStreamKind {
    fn input_key(self, device_id: &str, channel_id: &str, profile: &str) -> Option<String> {
        matches!(self, Self::Live).then(|| format!("live:{device_id}:{channel_id}:{profile}"))
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Playback => "playback",
            Self::Download => "download",
            Self::Broadcast => "broadcast",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Live => "start_live",
            Self::Playback => "start_playback",
            Self::Download => "start_download",
            Self::Broadcast => "start_broadcast",
        }
    }

    fn session_capability(self) -> &'static str {
        match self {
            Self::Live => "device.live",
            Self::Playback => "device.playback",
            Self::Download => "device.download",
            Self::Broadcast => "device.broadcast",
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gmv_protocol::common::v1::ErrorDetail;

    use super::*;

    #[test]
    fn live_input_keys_isolate_main_and_sub_profiles() {
        assert_eq!(
            DeviceStreamKind::Live.input_key("device", "channel", "main"),
            Some("live:device:channel:main".to_string())
        );
        assert_eq!(
            DeviceStreamKind::Live.input_key("device", "channel", "sub"),
            Some("live:device:channel:sub".to_string())
        );
        assert_ne!(
            DeviceStreamKind::Live.input_key("device", "channel", "main"),
            DeviceStreamKind::Live.input_key("device", "channel", "sub")
        );
    }

    #[test]
    fn live_input_owner_claim_is_atomic() {
        let store = InMemoryGuardStore::default();
        let first = store.claim_stream_input_owner(
            "live:device:channel",
            StreamSessionOwnerRecord {
                stream_id: String::new(),
                input_key: "live:device:channel".to_string(),
                node_id: "session-a".to_string(),
                instance_id: "instance-a".to_string(),
            },
        );
        let second = store.claim_stream_input_owner(
            "live:device:channel",
            StreamSessionOwnerRecord {
                stream_id: String::new(),
                input_key: "live:device:channel".to_string(),
                node_id: "session-b".to_string(),
                instance_id: "instance-b".to_string(),
            },
        );

        assert_eq!(first.node_id, "session-a");
        assert_eq!(second, first);

        store.upsert_stream_session_owner(StreamSessionOwnerRecord {
            stream_id: "stream-live".to_string(),
            input_key: "live:device:channel".to_string(),
            node_id: "session-a".to_string(),
            instance_id: "instance-a".to_string(),
        });
        store.remove_stream_session_owner("stream-live");
        let inactive = store
            .get_stream_session_owner_by_input("live:device:channel")
            .unwrap();
        assert!(inactive.stream_id.is_empty());
        assert_eq!(inactive.node_id, "session-a");

        let replacement = store.replace_inactive_stream_input_owner(
            "live:device:channel",
            StreamSessionOwnerRecord {
                stream_id: String::new(),
                input_key: "live:device:channel".to_string(),
                node_id: "session-b".to_string(),
                instance_id: "instance-b".to_string(),
            },
        );
        assert_eq!(replacement.node_id, "session-b");
    }

    #[test]
    fn remote_error_uses_global_code_metadata_for_user_message() {
        let error = ErrorDetail {
            code: "session_business_failed".to_string(),
            message: "stream input timeout".to_string(),
            metadata: HashMap::from([(
                META_GLOBAL_CODE.to_string(),
                (GmvGuardErrorCode::StreamInputTimeout as u16).to_string(),
            )]),
        };

        let error = remote_error(
            "session",
            "start_live",
            error,
            "stream_start_failed",
            "视频流创建失败，请检查设备在线状态和媒体服务",
            false,
        );

        let GuardError::UserVisible {
            code,
            user_message,
            retryable,
            details,
            ..
        } = error
        else {
            panic!("expected user-visible error");
        };
        assert_eq!(code, "stream_input_timeout");
        assert_eq!(
            user_message,
            GmvGuardErrorCode::StreamInputTimeout.out_msg()
        );
        assert!(retryable);
        assert_eq!(
            details.get("remote_code").map(String::as_str),
            Some("session_business_failed")
        );
    }

    #[test]
    fn remote_error_keeps_legacy_detail_code_without_global_metadata() {
        let error = ErrorDetail {
            code: "snapshot_rejected".to_string(),
            message: "device rejected snapshot".to_string(),
            metadata: HashMap::new(),
        };

        let error = remote_error(
            "session",
            "snapshot_image",
            error,
            "snapshot_rejected",
            "抓拍请求未被设备接受，请确认设备在线且支持抓拍",
            true,
        );

        let GuardError::UserVisible {
            code,
            user_message,
            retryable,
            details,
            ..
        } = error
        else {
            panic!("expected user-visible error");
        };
        assert_eq!(code, "snapshot_rejected");
        assert_eq!(user_message, GmvGuardErrorCode::SnapshotRejected.out_msg());
        assert!(retryable);
        assert_eq!(
            details.get("remote_code").map(String::as_str),
            Some("snapshot_rejected")
        );
    }

    #[test]
    fn broadcast_transport_and_parent_state_are_strictly_aggregated() {
        assert_eq!(normalize_broadcast_transport("").unwrap(), "udp");
        assert_eq!(
            normalize_broadcast_transport("tcp_active").unwrap(),
            "tcp_active"
        );
        assert!(normalize_broadcast_transport("tcp").is_err());

        let target = |state: &str| BroadcastTargetRecord {
            target_key: state.to_string(),
            device_id: "device".to_string(),
            channel_id: state.to_string(),
            session_node_id: "session".to_string(),
            leg_id: state.to_string(),
            transport: "udp".to_string(),
            profile: "raw_g711".to_string(),
            state: state.to_string(),
            reason: String::new(),
        };
        assert_eq!(
            aggregate_broadcast_start_state(&[target("running"), target("running")]),
            "running"
        );
        assert_eq!(
            aggregate_broadcast_start_state(&[target("running"), target("failed")]),
            "partial"
        );
        assert_eq!(
            aggregate_broadcast_start_state(&[target("failed")]),
            "failed"
        );
        assert_eq!(
            aggregate_broadcast_runtime_state(&[target("stopped"), target("failed")]),
            "stopped"
        );
    }
}
