use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error as log_error, info};
use base::serde::de::DeserializeOwned;
use base_rpc::RpcChannelConfig;
use gmv_domain::info::obj::{
    OutputStreamInfo, RegisterStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState,
    TalkClosedEvent, UnknownStreamEvent,
};
use gmv_nodec::NodeEventSender;
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity, NodeKind, OperationRef, ResourceRef,
};
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, AllocateStreamResponse, EventPriority, FinishRecordRequest, NodeEvent,
    NodeHealth, NodeHeartbeat, NodeResourceSnapshot, NodeToGuardMessage, QueryRunningRecordRequest,
    RecordMutationResponse, RegisterNodeRequest, ResourceReport, ResourceState, StartRecordRequest,
    guard_media_client::GuardMediaClient, node_to_guard_message,
};
use gmv_protocol::session::v1::{
    ControlPtzRequest, ControlPtzResponse, DeviceStreamResponse, DeviceStreamState,
    SessionHookRequest, SessionHookResponse, StartDeviceStreamRequest, StopDeviceStreamRequest,
    session_control_server::SessionControl, session_hook_server::SessionHook,
};
use gmv_protocol::stream::v1::{
    StartReceiveRequest, StartReceiveResponse, StreamState as ProtoStreamState,
};
use tonic::transport::Channel;

use crate::service::hook_serv;

static GUARD_EVENT_SENDER: OnceLock<NodeEventSender> = OnceLock::new();
static GUARD_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn init_guard_event_sender(sender: NodeEventSender) {
    let _ = GUARD_EVENT_SENDER.set(sender);
}

async fn guard_media_client() -> GlobalResult<GuardMediaClient<Channel>> {
    let endpoint = std::env::var("GMV_GUARD_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:18080".to_string());
    let channel = base_rpc::connect_channel(&RpcChannelConfig::new(endpoint.clone()))
        .await
        .map_err(|err| {
            GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "connect guard media rpc failed",
                |msg| log_error!("{msg}: endpoint={endpoint}, err={err:?}"),
            )
        })?;
    Ok(GuardMediaClient::new(channel))
}

pub async fn guard_record_running(device_id: &str, channel_id: &str) -> GlobalResult<bool> {
    let mut client = guard_media_client().await?;
    let response = client
        .query_running_record(QueryRunningRecordRequest {
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    Ok(response.exists)
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
    let mut client = guard_media_client().await?;
    let response = client
        .start_record(StartRecordRequest {
            biz_id: biz_id.to_string(),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            user_id: String::new(),
            st_epoch_sec,
            et_epoch_sec,
            speed,
            stream_app_name: stream_app_name.to_string(),
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    ensure_record_mutation(response, "start_record")
}

pub async fn guard_record_finished(
    biz_id: &str,
    file_size: u64,
    record_duration_sec: u64,
    file_format: &str,
    dir_path: &str,
    abs_path: &str,
) -> GlobalResult<()> {
    let mut client = guard_media_client().await?;
    let response = client
        .finish_record(FinishRecordRequest {
            biz_id: biz_id.to_string(),
            file_size,
            record_duration_sec,
            file_format: file_format.to_string(),
            dir_path: dir_path.to_string(),
            abs_path: abs_path.to_string(),
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    ensure_record_mutation(response, "finish_record")
}

fn ensure_record_mutation(response: RecordMutationResponse, action: &str) -> GlobalResult<()> {
    if response.accepted && response.error.is_none() {
        return Ok(());
    }
    let message = response
        .error
        .as_ref()
        .map(|error| error.message.as_str())
        .filter(|message| !message.is_empty())
        .unwrap_or("guard media rpc failed");
    Err(GlobalError::new_biz_error(
        BaseErrorCode::Internal.code(),
        message,
        |msg| log_error!("guard media rpc {action} failed: {msg}"),
    ))
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
    pub fn new(node_id: impl Into<String>, instance_id: impl Into<String>, http_port: u32) -> Self {
        Self {
            guard_channel: RpcChannelConfig::new("http://127.0.0.1:18080"),
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Session as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            endpoints: vec![Endpoint {
                name: "http".to_string(),
                scheme: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: http_port,
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            }],
            capabilities: vec![
                "device.live".to_string(),
                "device.playback".to_string(),
                "device.download".to_string(),
                "device.talk".to_string(),
                "device.ptz".to_string(),
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
            capacity: 100,
            zone: String::new(),
            takeover: false,
        }
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

    async fn start_talk(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        self.start_device_stream(request, "talk").await
    }

    async fn stop_device_stream(
        &self,
        request: tonic::Request<StopDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.stop_device_stream(request.into_inner()),
        ))
    }

    async fn control_ptz(
        &self,
        request: tonic::Request<ControlPtzRequest>,
    ) -> Result<tonic::Response<ControlPtzResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.control_ptz(request.into_inner()),
        ))
    }
}

impl SessionControlRpc {
    async fn start_device_stream(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
        stream_type: &str,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.start_device_stream(request.into_inner(), stream_type),
        ))
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
                hook_response(true, Some(hook_serv::stream_input_timeout(value)))?
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
                        error: Some(error("end_record_failed", &err.to_string())),
                    },
                }
            }
            "stream.talk_closed" => {
                let value: TalkClosedEvent = decode_payload(&request.payload_json)?;
                hook_response(hook_serv::talk_closed(value).await, None::<()>)?
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
        self.active_streams
            .entry(stream_id.clone())
            .or_insert(SessionStream {
                device_id: request.device_id,
                channel_id: request.channel_id,
                route_id: request.route_id,
                lease_id: request.lease_id,
                state: DeviceStreamState::Running,
            });
        device_response(&stream_id, DeviceStreamState::Running, None)
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
        if let Some(existing) = self.active_streams.get(&stream_id) {
            if existing.lease_id == request.lease_id {
                return device_response(&stream_id, existing.state, None);
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
                state: DeviceStreamState::Running,
            },
        );
        device_response(&stream_id, DeviceStreamState::Running, None)
    }

    pub fn stop_device_stream(&mut self, request: StopDeviceStreamRequest) -> DeviceStreamResponse {
        match self.active_streams.get_mut(&request.stream_id) {
            Some(stream) => {
                stream.state = DeviceStreamState::Stopped;
                device_response(&request.stream_id, DeviceStreamState::Stopped, None)
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
                payload: format!("stream_id={stream_id};fallback=legacy_http").into_bytes(),
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
    }
}

fn error(code: &str, message: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: message.to_string(),
        metadata: HashMap::new(),
    }
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
        let node = SessionGuardNode::new("session-1", "inst-1", 18081);
        let register = node.register_request(NodeResourceSnapshot {
            resources: vec![],
            full: true,
        });
        assert_eq!(register.identity.unwrap().kind, NodeKind::Session as i32);
        assert!(register.capabilities.contains(&"device.ptz".to_string()));

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
            },
            StartReceiveResponse {
                stream_id: "stream-1".to_string(),
                state: ProtoStreamState::Receiving as i32,
                receive_endpoints: vec![],
                error: None,
            },
        );
        assert_eq!(response.state, DeviceStreamState::Running as i32);
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
        let node = SessionGuardNode::new("session-1", "inst-1", 18081);
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
            control
                .guard_unavailable_event("op-1", "legacy-stream")
                .payload,
            Some(node_to_guard_message::Payload::Event(_))
        ));
    }
}
