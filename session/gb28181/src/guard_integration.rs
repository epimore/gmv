use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error as log_error, info, warn};
use base::serde::de::DeserializeOwned;
use base_rpc::RpcChannelConfig;
use gmv_domain::info::format::{CMaf, Flv};
use gmv_domain::info::obj::{
    OutputStreamInfo, RegisterStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState,
    TalkClosedEvent, TalkStartModel, TalkStopModel, UnknownStreamEvent,
};
use gmv_domain::info::output::{DashFmp4Output, HttpFlvOutput, OutputKind};
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
    ControlPtzRequest, ControlPtzResponse, DeviceStreamResponse, DeviceStreamState, GbChannel,
    GbChannelImage, GbDevice, GetGbChannelRequest, GetGbChannelResponse, GetGbDeviceRequest,
    GetGbDeviceResponse, ListGbChannelImagesRequest, ListGbChannelImagesResponse,
    ListGbChannelsRequest, ListGbChannelsResponse, ListGbDevicesRequest, ListGbDevicesResponse,
    SessionHookRequest, SessionHookResponse, StartDeviceStreamRequest, StopDeviceStreamRequest,
    session_control_server::SessionControl, session_hook_server::SessionHook,
};
use gmv_protocol::stream::v1::{
    StartReceiveRequest, StartReceiveResponse, StreamState as ProtoStreamState,
};
use tonic::transport::Channel;

use crate::service::{api_serv, hook_serv, stream_close};
use crate::state::model::{PlayBackModel, PlayLiveModel, PtzControlModel, TransMode};
use crate::state::session::GuardLease;
use crate::state::{StreamNode, StreamNodeRegistry};

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
    let channel = base_rpc::connect_channel(&rpc_channel_config(endpoint.clone()))
        .await
        .map_err(|err| {
            GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "connect guard control rpc failed",
                |msg| log_error!("{msg}: endpoint={endpoint}, err={err:?}"),
            )
        })?;
    Ok(GuardControlClient::new(channel))
}

#[derive(Debug, Clone)]
pub struct AllocatedStreamNode {
    pub node: StreamNode,
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
    let mut client = guard_control_client().await?;
    let response = client
        .allocate_stream(AllocateStreamRequest {
            operation: Some(operation(operation_id)),
            stream_id: stream_id.to_string(),
            stream_type: stream_type.to_string(),
            constraints: HashMap::from([
                ("device_id".to_string(), device_id.to_string()),
                ("channel_id".to_string(), channel_id.to_string()),
            ]),
        })
        .await
        .hand_log(|msg| log_error!("{msg}"))?
        .into_inner();
    let node = stream_node_from_allocation(&response)?;
    StreamNodeRegistry::upsert(node.clone());
    Ok(AllocatedStreamNode {
        node,
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
    let node = stream_node_from_parts(&identity.node_id, response.endpoints)?;
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

fn stream_node_from_allocation(allocation: &AllocateStreamResponse) -> GlobalResult<StreamNode> {
    let identity = allocation.stream_node.as_ref().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "guard allocation response has no stream node",
            |msg| log_error!("{msg}: lease_id={}", allocation.lease_id),
        )
    })?;
    stream_node_from_parts(&identity.node_id, allocation.endpoints.clone())
}

fn stream_node_from_parts(node_id: &str, endpoints: Vec<Endpoint>) -> GlobalResult<StreamNode> {
    let grpc = endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| missing_stream_endpoint(node_id, "grpc"))?;
    let rtp = endpoints
        .iter()
        .find(|endpoint| endpoint.name == "rtp" || endpoint.scheme == "rtp")
        .ok_or_else(|| missing_stream_endpoint(node_id, "rtp"))?;
    let http = endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "http" || matches!(endpoint.scheme.as_str(), "http" | "https")
        })
        .unwrap_or(grpc);
    Ok(StreamNode {
        name: node_id.to_string(),
        local_ip: parse_ipv4(node_id, "http", &http.host)?,
        local_port: u16::try_from(http.port).unwrap_or(u16::MAX),
        control_grpc_uri: base_rpc::rpc_endpoint_uri(
            grpc.scheme == "grpcs",
            &grpc.host,
            u16::try_from(grpc.port).unwrap_or(u16::MAX),
        ),
        pub_ip: parse_ipv4(node_id, "rtp", &rtp.host)?,
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

fn parse_ipv4(node_id: &str, endpoint: &str, host: &str) -> GlobalResult<std::net::Ipv4Addr> {
    host.parse().map_err(|err| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "stream endpoint host must be an IPv4 address",
            |msg| {
                log_error!("{msg}: node={node_id}, endpoint={endpoint}, host={host}, err={err:?}")
            },
        )
    })
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
    file_size: u64,
    record_duration_sec: u64,
    file_format: &str,
    dir_path: &str,
    abs_path: &str,
) -> GlobalResult<()> {
    let finished =
        crate::storage::recording::finish_record(crate::storage::recording::RecordFinish {
            biz_id,
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
    pub fn new(node_id: impl Into<String>, instance_id: impl Into<String>, http_port: u32) -> Self {
        Self {
            guard_channel: rpc_channel_config(crate::state::GuardConf::get_or_default().endpoint),
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
            capacity: 100,
            zone: String::new(),
            takeover: false,
            config: self.config_summary(),
        }
    }

    fn config_summary(&self) -> HashMap<String, String> {
        HashMap::from([
            ("node_id".to_string(), self.identity.node_id.clone()),
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
            (
                "capability_count".to_string(),
                self.capabilities.len().to_string(),
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
        let request = request.into_inner();
        let stopped = if crate::state::session::Cache::talk_map_get(&request.stream_id).is_some() {
            api_serv::talk_stop(
                TalkStopModel {
                    talk_id: request.stream_id.clone(),
                },
                String::new(),
            )
            .await
            .map_err(device_error)
        } else {
            stream_close::begin(request.stream_id.clone());
            Ok(true)
        };
        let response = match stopped {
            Ok(_) => DeviceStreamResponse {
                stream_id: request.stream_id,
                state: DeviceStreamState::Stopped as i32,
                error: None,
                endpoint: String::new(),
            },
            Err(error) => error,
        };
        Ok(tonic::Response::new(response))
    }

    async fn control_ptz(
        &self,
        request: tonic::Request<ControlPtzRequest>,
    ) -> Result<tonic::Response<ControlPtzResponse>, tonic::Status> {
        let request = request.into_inner();
        let model = ptz_model(&request);
        let response = match api_serv::ptz(model, String::new()).await {
            Ok(_) => ControlPtzResponse {
                accepted: true,
                error: None,
            },
            Err(err) => ControlPtzResponse {
                accepted: false,
                error: Some(error("ptz_failed", &err.to_string())),
            },
        };
        Ok(tonic::Response::new(response))
    }

    async fn list_gb_devices(
        &self,
        _request: tonic::Request<ListGbDevicesRequest>,
    ) -> Result<tonic::Response<ListGbDevicesResponse>, tonic::Status> {
        let session_node_id = self.session_node_id()?;
        let devices = crate::storage::guard_query::GbDeviceView::list()
            .await
            .map_err(storage_status)?
            .into_iter()
            .map(|device| gb_device_proto(device, &session_node_id))
            .collect();
        Ok(tonic::Response::new(ListGbDevicesResponse { devices }))
    }

    async fn get_gb_device(
        &self,
        request: tonic::Request<GetGbDeviceRequest>,
    ) -> Result<tonic::Response<GetGbDeviceResponse>, tonic::Status> {
        let session_node_id = self.session_node_id()?;
        let request = request.into_inner();
        let device = crate::storage::guard_query::GbDeviceView::get(&request.device_id)
            .await
            .map_err(storage_status)?
            .map(|device| gb_device_proto(device, &session_node_id));
        Ok(tonic::Response::new(GetGbDeviceResponse { device }))
    }

    async fn list_gb_channels(
        &self,
        request: tonic::Request<ListGbChannelsRequest>,
    ) -> Result<tonic::Response<ListGbChannelsResponse>, tonic::Status> {
        let request = request.into_inner();
        let channels = crate::storage::guard_query::GbChannelView::list(&request.device_id)
            .await
            .map_err(storage_status)?
            .into_iter()
            .map(gb_channel_proto)
            .collect();
        Ok(tonic::Response::new(ListGbChannelsResponse { channels }))
    }

    async fn get_gb_channel(
        &self,
        request: tonic::Request<GetGbChannelRequest>,
    ) -> Result<tonic::Response<GetGbChannelResponse>, tonic::Status> {
        let request = request.into_inner();
        let channel = crate::storage::guard_query::GbChannelView::get(
            &request.device_id,
            &request.channel_id,
        )
        .await
        .map_err(storage_status)?
        .map(gb_channel_proto);
        Ok(tonic::Response::new(GetGbChannelResponse { channel }))
    }

    async fn list_gb_channel_images(
        &self,
        request: tonic::Request<ListGbChannelImagesRequest>,
    ) -> Result<tonic::Response<ListGbChannelImagesResponse>, tonic::Status> {
        let request = request.into_inner();
        let images = crate::storage::guard_query::GbChannelImageView::list(
            &request.device_id,
            &request.channel_id,
        )
        .await
        .map_err(storage_status)?
        .into_iter()
        .map(gb_channel_image_proto)
        .collect();
        Ok(tonic::Response::new(ListGbChannelImagesResponse { images }))
    }
}

impl SessionControlRpc {
    fn session_node_id(&self) -> Result<String, tonic::Status> {
        self.inner
            .lock()
            .map_err(|_| tonic::Status::internal("session control lock poisoned"))
            .map(|control| control.identity.node_id.clone())
    }
}

impl SessionControlRpc {
    async fn start_device_stream(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
        stream_type: &str,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        let request = request.into_inner();
        {
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
        }
        let token = if request.token.trim().is_empty() {
            operation_token(request.operation.as_ref())
        } else {
            request.token.clone()
        };
        let response = match stream_type {
            "live" => api_serv::play_live(
                PlayLiveModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: trans_mode(&request.trans_mode),
                    custom_media_config: custom_media_config(&request.output_type),
                },
                token,
            )
            .await
            .map(|info| stream_response(info.streamId, info.url)),
            "playback" => api_serv::play_back(
                PlayBackModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: trans_mode(&request.trans_mode),
                    custom_media_config: custom_media_config(&request.output_type),
                    st: request.start_time_sec,
                    et: request.end_time_sec,
                },
                token,
            )
            .await
            .map(|info| stream_response(info.streamId, info.url)),
            "download" => api_serv::download(
                PlayBackModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    trans_mode: trans_mode(&request.trans_mode),
                    custom_media_config: None,
                    st: request.start_time_sec,
                    et: request.end_time_sec,
                },
                token,
            )
            .await
            .map(|stream_id| stream_response(stream_id, String::new())),
            "talk" => api_serv::talk_start(
                TalkStartModel {
                    device_id: request.device_id.clone(),
                    channel_id: optional_channel(&request.channel_id),
                    transport: empty_to_none(request.trans_mode.clone()),
                    codec: empty_to_none(request.talk_codec.clone()),
                    sample_rate: non_zero(request.talk_sample_rate),
                    channel_count: u8_non_zero(request.talk_channel_count),
                    frame_duration_ms: u16_non_zero(request.talk_frame_duration_ms),
                },
                token,
            )
            .await
            .map(|info| stream_response(info.talk_id, info.input_url)),
            _ => Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "unsupported stream type",
                |msg| log_error!("{msg}: {stream_type}"),
            )),
        }
        .unwrap_or_else(device_error);
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
    }
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
        channel_count: row.channel_count.try_into().unwrap_or(u32::MAX),
    }
}

fn gb_channel_proto(row: crate::storage::guard_query::GbChannelView) -> GbChannel {
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
        talk_enable: row.talk_enable,
        audio_enable: row.audio_enable,
        record_enable: row.record_enable,
        playback_enable: row.playback_enable,
        alarm_enable: row.alarm_enable,
        biz_enable: row.biz_enable,
        sort_no: row.sort_no,
        created_at_ms: datetime_ms(row.created_at),
        updated_at_ms: datetime_ms(row.updated_at),
    }
}

fn gb_channel_image_proto(row: crate::storage::guard_query::GbChannelImageView) -> GbChannelImage {
    GbChannelImage {
        image_id: row.image_id,
        device_id: row.device_id,
        channel_id: row.channel_id,
        image_url: row.image_url,
        created_at_ms: datetime_ms(row.created_at),
    }
}

fn datetime_string(value: Option<base::chrono::NaiveDateTime>) -> String {
    value
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn datetime_ms(value: Option<base::chrono::NaiveDateTime>) -> i64 {
    value
        .map(|value| value.and_utc().timestamp_millis())
        .unwrap_or_default()
}

fn storage_status(error: GlobalError) -> tonic::Status {
    tonic::Status::internal(error.to_string())
}

fn stream_response(stream_id: String, endpoint: String) -> DeviceStreamResponse {
    DeviceStreamResponse {
        stream_id,
        state: DeviceStreamState::Running as i32,
        error: None,
        endpoint,
    }
}

fn device_error(err: GlobalError) -> DeviceStreamResponse {
    DeviceStreamResponse {
        stream_id: String::new(),
        state: DeviceStreamState::Failed as i32,
        error: Some(error("session_business_failed", &err.to_string())),
        endpoint: String::new(),
    }
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

fn trans_mode(value: &str) -> Option<TransMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "udp" => Some(TransMode::Udp),
        "tcp_active" | "tcpactive" | "tcp-active" => Some(TransMode::TcpActive),
        "tcp_passive" | "tcppassive" | "tcp-passive" => Some(TransMode::TcpPassive),
        _ => None,
    }
}

fn custom_media_config(output_type: &str) -> Option<crate::state::model::CustomMediaConfig> {
    let output = match output_type.trim().to_ascii_lowercase().as_str() {
        "" => return None,
        "http_flv" | "flv" => OutputKind::HttpFlv(HttpFlvOutput {
            fmt: Flv::default(),
        }),
        "dash_fmp4" | "fmp4" => OutputKind::DashFmp4(DashFmp4Output {
            fmt: CMaf::default(),
        }),
        _ => return None,
    };
    Some(crate::state::model::CustomMediaConfig {
        output,
        codec: None,
        filter: Default::default(),
    })
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
        "zoom_out" | "out" => model.inOut = 1,
        "zoom_in" | "in" => model.inOut = 2,
        _ => {}
    }
    model
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
}
