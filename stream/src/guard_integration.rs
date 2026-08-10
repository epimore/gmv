use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::serde::{Serialize, de::DeserializeOwned};
use base::serde_json;
use base::tokio::sync::{Mutex, mpsc, oneshot};
use base_rpc::RpcChannelConfig;
use gmv_domain::info::media_info::MediaConfig;
use gmv_domain::info::media_info_ext::MediaMap;
use gmv_domain::info::obj::{
    BroadcastCloseReq, BroadcastConfigureLegReq, BroadcastOpenReq, StreamInfoQo, StreamKey,
    StreamRecordInfo,
};
use gmv_domain::info::output::{OutputEnum, OutputKind};
use gmv_nodec::NodeEventSender;
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity, NodeKind, OperationRef, ResourceRef,
};
use gmv_protocol::guard::v1::{
    CheckPlaybackRequest, EventPriority, NodeEvent, NodeHealth, NodeHeartbeat,
    NodeResourceSnapshot, NodeToGuardMessage, RegisterNodeRequest, ResourceReport, ResourceState,
    guard_control_client::GuardControlClient, node_to_guard_message,
};
use gmv_protocol::stream::v1::{
    CloseOutputRequest, CloseOutputResponse, ConfigureReceiveTransportRequest,
    ConfigureReceiveTransportResponse, CreateOutputRequest, CreateOutputResponse,
    GetPlaybackEndpointsRequest, GetPlaybackEndpointsResponse, MediaReadinessStage, MediaTransport,
    MediaTransportState, OutputInfo, OutputState, QueryStreamRequest, QueryStreamResponse,
    ReleaseSubscriptionOutputsRequest, ReleaseSubscriptionOutputsResponse, StartReceiveRequest,
    StartReceiveResponse, StopReceivePhase, StopReceiveRequest, StopReceiveResponse,
    StreamBoolResponse, StreamJsonRequest, StreamJsonResponse, StreamState, StreamUnitResponse,
    ViewerFormatCount, stream_control_server::StreamControl,
};
use tonic::transport::Channel;

use crate::io::broadcast::BroadcastManager;
use crate::io::local::mp4::Mp4OutputInnerEvent;
use crate::io::media_endpoint::{
    ConnectMediaEndpoint, MediaConnectionState, MediaEndpointManager, ReserveMediaEndpoint,
};
use crate::state::register::{
    FinalizeStreamResult, OutputRuntimeState, Register, StreamRuntimeObservation,
};

static GUARD_EVENT_SENDER: OnceLock<NodeEventSender> = OnceLock::new();
static GUARD_CHANNEL: OnceLock<RpcChannelConfig> = OnceLock::new();
static GUARD_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TERMINAL_SUBSCRIPTION_RETENTION_MS: i64 = 3_600_000;

pub fn init_guard_event_sender(sender: NodeEventSender) {
    let _ = GUARD_EVENT_SENDER.set(sender);
}

pub fn init_guard_channel(channel: RpcChannelConfig) {
    let _ = GUARD_CHANNEL.set(channel);
}

async fn guard_control_client() -> GlobalResult<GuardControlClient<Channel>> {
    let Some(channel_config) = GUARD_CHANNEL.get() else {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "guard control rpc channel is not initialized",
            |msg| base::log::error!("{msg}"),
        ));
    };
    let started = Instant::now();
    base::log::debug!(
        "stream rpc client outbound: service=guard_control, endpoint={}",
        channel_config.endpoint
    );
    let channel = base_rpc::connect_channel(channel_config)
        .await
        .map_err(|err| {
            base::log::debug!(
                "stream rpc client inbound: service=guard_control, endpoint={}, status=error, elapsed_ms={}, err={err:?}",
                channel_config.endpoint,
                started.elapsed().as_millis()
            );
            GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "connect guard control rpc failed",
                |msg| base::log::error!("{msg}: endpoint={}, err={err:?}", channel_config.endpoint),
            )
        })?;
    base::log::debug!(
        "stream rpc client inbound: service=guard_control, endpoint={}, status=ok, elapsed_ms={}",
        channel_config.endpoint,
        started.elapsed().as_millis()
    );
    Ok(GuardControlClient::new(channel))
}

pub async fn check_playback(
    stream_id: &str,
    token: &str,
    remote_addr: Option<&str>,
    output_type: &str,
) -> bool {
    let mut client = match guard_control_client().await {
        Ok(client) => client,
        Err(err) => {
            base::log::warn!("guard playback check skipped: stream_id={stream_id}, err={err}");
            return false;
        }
    };
    base::log::debug!(
        "stream rpc client outbound: method=guard_control.check_playback, req: stream_id={}, token={}, remote_addr={}, output_type={}",
        stream_id,
        if token.is_empty() {
            "<empty>"
        } else {
            "<redacted>"
        },
        remote_addr.unwrap_or_default(),
        output_type
    );
    match client
        .check_playback(tonic::Request::new(CheckPlaybackRequest {
            stream_id: stream_id.to_string(),
            token: token.to_string(),
            remote_addr: remote_addr.unwrap_or_default().to_string(),
            output_type: output_type.to_string(),
        }))
        .await
    {
        Ok(response) => {
            let response = response.into_inner();
            if !response.accepted {
                base::log::warn!(
                    "guard playback rejected: stream_id={stream_id}, output_type={output_type}, error={:?}",
                    response.error
                );
            }
            response.accepted
        }
        Err(err) => {
            base::log::warn!(
                "guard playback check failed: stream_id={stream_id}, output_type={output_type}, err={err:?}"
            );
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardEventPublish {
    Queued,
    Unavailable,
    Full,
    Closed,
}

pub fn publish_guard_event(topic: &str, payload: impl Into<Vec<u8>>) -> GuardEventPublish {
    let payload = payload.into();
    let Some(sender) = GUARD_EVENT_SENDER.get() else {
        base::log::warn!(
            "guard event outbound skipped: topic={topic}, reason=event_sender_not_initialized, payload_bytes={}",
            payload.len()
        );
        return GuardEventPublish::Unavailable;
    };
    let sequence = GUARD_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let event_id = format!("stream-event-{sequence}");
    let event = NodeEvent {
        event_id: event_id.clone(),
        topic: topic.to_string(),
        priority: EventPriority::P1 as i32,
        payload,
    };
    match sender.try_send(event) {
        Ok(()) => GuardEventPublish::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => {
            base::log::warn!(
                "guard event outbound failed: event_id={event_id}, topic={topic}, outcome=full"
            );
            GuardEventPublish::Full
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            base::log::warn!(
                "guard event outbound failed: event_id={event_id}, topic={topic}, outcome=closed"
            );
            GuardEventPublish::Closed
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamGuardNode {
    pub guard_channel: RpcChannelConfig,
    pub identity: NodeIdentity,
    pub software_version: String,
    pub started_at_epoch_ms: i64,
    pub endpoints: Vec<Endpoint>,
    pub capabilities: Vec<String>,
    pub host_id: String,
}

impl StreamGuardNode {
    pub fn new(
        node_id: impl Into<String>,
        instance_id: impl Into<String>,
        host: impl Into<String>,
        guard_endpoint: impl Into<String>,
        http_port: u32,
        http_tls: bool,
        rtp_port: u32,
    ) -> Self {
        let host = host.into();
        let guard_endpoint = guard_endpoint.into();
        let mut guard_channel = RpcChannelConfig::new(guard_endpoint.clone());
        if guard_endpoint.starts_with("https://") {
            guard_channel.tls = Some(base_rpc::RpcClientTlsConfig {
                domain_name: url::Url::parse(&guard_endpoint)
                    .ok()
                    .and_then(|url| url.host_str().map(ToString::to_string)),
                ca_certificate_pem: None,
                client_certificate_pem: None,
                client_private_key_pem: None,
                use_native_roots: true,
                handshake_timeout: std::time::Duration::from_secs(5),
            });
        }
        Self {
            guard_channel,
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Stream as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            host_id: host.clone(),
            endpoints: vec![
                endpoint(
                    "http",
                    if http_tls { "https" } else { "http" },
                    &host,
                    http_port,
                ),
                endpoint("rtp", "rtp", &host, rtp_port),
            ],
            capabilities: vec![
                "live".to_string(),
                "playback".to_string(),
                "download".to_string(),
                "broadcast".to_string(),
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
            takeover: cfg!(debug_assertions),
            config: self.config_summary(),
        }
    }

    fn config_summary(&self) -> HashMap<String, String> {
        HashMap::from([
            ("node_id".to_string(), self.identity.node_id.clone()),
            ("host_id".to_string(), self.host_id.clone()),
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
        receiving: usize,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence,
            sent_at_epoch_ms,
            payload: Some(node_to_guard_message::Payload::Heartbeat(NodeHeartbeat {
                health: NodeHealth::Ready as i32,
                host_metrics: None,
                metrics: HashMap::from([("receiving_streams".to_string(), receiving.to_string())]),
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

    pub fn frame_ready_event(
        &self,
        sequence: u64,
        sent_at_epoch_ms: i64,
        stream_id: &str,
        frame_ref: &str,
        ttl_ms: u64,
    ) -> NodeToGuardMessage {
        let payload =
            format!("stream_id={stream_id};frame_ref={frame_ref};ttl_ms={ttl_ms}").into_bytes();
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence,
            sent_at_epoch_ms,
            payload: Some(node_to_guard_message::Payload::Event(NodeEvent {
                event_id: format!("frame-{stream_id}-{sequence}"),
                topic: "stream.frame.ready".to_string(),
                priority: EventPriority::P2 as i32,
                payload,
            })),
        }
    }
}

#[derive(Clone)]
pub struct StreamControlRpc {
    inner: Arc<Mutex<StreamControlAdapter>>,
}

impl StreamControlRpc {
    pub fn new(adapter: StreamControlAdapter) -> Self {
        Self {
            inner: Arc::new(Mutex::new(adapter)),
        }
    }

    pub async fn resource_snapshot(&self) -> NodeResourceSnapshot {
        self.inner.lock().await.resource_snapshot()
    }
}

#[tonic::async_trait]
impl StreamControl for StreamControlRpc {
    async fn start_receive(
        &self,
        request: tonic::Request<StartReceiveRequest>,
    ) -> Result<tonic::Response<StartReceiveResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.start_receive, req:{request:?}");
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(control.start_receive(request).await))
    }

    async fn configure_receive_transport(
        &self,
        request: tonic::Request<ConfigureReceiveTransportRequest>,
    ) -> Result<tonic::Response<ConfigureReceiveTransportResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.configure_receive_transport, req: stream_id={}, endpoint_id={}, generation={}, transport={}",
            request.stream_id,
            request.endpoint_id,
            request.endpoint_generation,
            request.media_transport
        );
        let control = self.inner.lock().await;
        Ok(tonic::Response::new(
            control.configure_receive_transport(request).await,
        ))
    }

    async fn stop_receive(
        &self,
        request: tonic::Request<StopReceiveRequest>,
    ) -> Result<tonic::Response<StopReceiveResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.stop_receive, req:{request:?}");
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(control.stop_receive(request).await))
    }

    async fn query_stream(
        &self,
        request: tonic::Request<QueryStreamRequest>,
    ) -> Result<tonic::Response<QueryStreamResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.query_stream, req:{request:?}");
        let control = self.inner.lock().await;
        Ok(tonic::Response::new(control.query_stream(request)))
    }

    async fn create_output(
        &self,
        request: tonic::Request<CreateOutputRequest>,
    ) -> Result<tonic::Response<CreateOutputResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.create_output, req:{request:?}");
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(control.create_output(request)))
    }

    async fn close_output(
        &self,
        request: tonic::Request<CloseOutputRequest>,
    ) -> Result<tonic::Response<CloseOutputResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.close_output, req:{request:?}");
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(control.close_output(request)))
    }

    async fn release_subscription_outputs(
        &self,
        request: tonic::Request<ReleaseSubscriptionOutputsRequest>,
    ) -> Result<tonic::Response<ReleaseSubscriptionOutputsResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.release_subscription_outputs, req: stream_id={}, subscription_id={}",
            request.stream_id,
            request.subscription_id
        );
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(
            control.release_subscription_outputs(request),
        ))
    }

    async fn get_playback_endpoints(
        &self,
        request: tonic::Request<GetPlaybackEndpointsRequest>,
    ) -> Result<tonic::Response<GetPlaybackEndpointsResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.get_playback_endpoints, req:{request:?}");
        let control = self.inner.lock().await;
        Ok(tonic::Response::new(
            control.get_playback_endpoints(request),
        ))
    }

    async fn init_media(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.init_media, req: payload_bytes={}",
            request.payload_json.len()
        );
        let mut control = self.inner.lock().await;
        Ok(tonic::Response::new(control.init_media(request)))
    }

    async fn init_media_ext(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.init_media_ext, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<MediaMap>(&request.payload_json).and_then(|value| {
                Register::init_media_ext(value.ssrc, value.ext).map_err(detail_from_error)
            }),
        )))
    }

    async fn stream_online(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.stream_online, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(
            match decode_payload::<StreamKey>(&request.payload_json) {
                Ok(value) => StreamBoolResponse {
                    value: Register::is_exist(value),
                    error: None,
                },
                Err(error) => StreamBoolResponse {
                    value: false,
                    error: Some(error),
                },
            },
        ))
    }

    async fn record_info(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.record_info, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(record_info_response(request).await))
    }

    async fn close_output_by_ssrc(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.close_output_by_ssrc, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<StreamInfoQo>(&request.payload_json).and_then(close_output_by_ssrc),
        )))
    }

    async fn broadcast_open(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.broadcast_open, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(
            match decode_payload::<BroadcastOpenReq>(&request.payload_json) {
                Ok(value) => match BroadcastManager::open(value).await {
                    Ok(response) => json_response(&response),
                    Err(error) => StreamJsonResponse {
                        payload_json: vec![],
                        error: Some(detail_from_error(error)),
                    },
                },
                Err(error) => StreamJsonResponse {
                    payload_json: vec![],
                    error: Some(error),
                },
            },
        ))
    }

    async fn broadcast_configure_leg(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.broadcast_configure_leg, req: payload_bytes={}",
            request.payload_json.len()
        );
        let result = match decode_payload::<BroadcastConfigureLegReq>(&request.payload_json) {
            Ok(value) => BroadcastManager::configure_leg(value)
                .await
                .map_err(detail_from_error),
            Err(error) => Err(error),
        };
        Ok(tonic::Response::new(stream_unit_response(result)))
    }

    async fn broadcast_close(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.broadcast_close, req: payload_bytes={}",
            request.payload_json.len()
        );
        let result = match decode_payload::<BroadcastCloseReq>(&request.payload_json) {
            Ok(value) => match BroadcastManager::close(&value.broadcast_id, &value.leg_id).await {
                Ok(true) => {
                    if value.leg_id.is_empty() || !BroadcastManager::is_online(&value.broadcast_id)
                    {
                        self.inner.lock().await.streams.remove(&value.broadcast_id);
                    }
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(error) => Err(detail_from_error(error)),
            },
            Err(error) => Err(error),
        };
        Ok(tonic::Response::new(stream_unit_response(result)))
    }

    async fn broadcast_online(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.broadcast_online, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(
            match decode_payload::<BroadcastCloseReq>(&request.payload_json) {
                Ok(value) => match BroadcastManager::wait_ready(
                    &value.broadcast_id,
                    &value.leg_id,
                    std::time::Duration::from_secs(5),
                )
                .await
                {
                    Ok(value) => StreamBoolResponse { value, error: None },
                    Err(error) => StreamBoolResponse {
                        value: false,
                        error: Some(detail_from_error(error)),
                    },
                },
                Err(error) => StreamBoolResponse {
                    value: false,
                    error: Some(error),
                },
            },
        ))
    }
}

#[derive(Clone)]
pub struct StreamControlAdapter {
    identity: NodeIdentity,
    receive_endpoint: Endpoint,
    streams: HashMap<String, StreamRuntime>,
    outputs: HashMap<String, OutputRuntime>,
    terminal_subscriptions: HashMap<(String, String), i64>,
    finalized_streams: HashMap<String, FinalizedStreamRuntime>,
    restart_close_watches: HashMap<String, RestartCloseWatch>,
    media_tx: Option<mpsc::Sender<u32>>,
    media_endpoints: Option<Arc<MediaEndpointManager>>,
}

#[derive(Debug, Clone)]
struct StreamRuntime {
    lease_id: String,
    route_id: String,
    endpoints: Vec<Endpoint>,
    state: StreamState,
    primary_output_format: String,
}

#[derive(Debug, Clone)]
struct FinalizedStreamRuntime {
    ssrc: u32,
    lifecycle_generation: u64,
    last_packet_at_ms: u64,
    packet_count: u64,
    input_idle_timeout_ms: u64,
    finalized_at_ms: i64,
}

#[derive(Debug, Clone)]
struct RestartCloseWatch {
    ssrc: u32,
    lifecycle_generation: u64,
    started_at_ms: u64,
    input_idle_timeout_ms: u64,
}

#[derive(Debug, Clone)]
struct OutputRuntime {
    output_id: String,
    stream_id: String,
    output_type: String,
    endpoint: String,
    state: OutputState,
    subscription_id: String,
}

impl OutputRuntime {
    fn info(&self, use_media_runtime: bool) -> OutputInfo {
        let metadata = use_media_runtime
            .then(|| Register::output_media_metadata(&self.stream_id, &self.output_type))
            .flatten();
        OutputInfo {
            output_id: self.output_id.clone(),
            stream_id: self.stream_id.clone(),
            output_type: self.output_type.clone(),
            endpoint: self.endpoint.clone(),
            state: metadata
                .as_ref()
                .map(|metadata| output_state(metadata.state))
                .unwrap_or(self.state) as i32,
            subscription_id: self.subscription_id.clone(),
            video_codec: metadata
                .as_ref()
                .map(|metadata| metadata.video_codec.clone())
                .unwrap_or_default(),
            audio_codec: metadata
                .as_ref()
                .map(|metadata| metadata.audio_codec.clone())
                .unwrap_or_default(),
            mime_codec: metadata
                .as_ref()
                .map(|metadata| metadata.mime_codec.clone())
                .unwrap_or_default(),
            failure: metadata.and_then(|metadata| metadata.failure),
        }
    }
}

fn output_state(state: OutputRuntimeState) -> OutputState {
    match state {
        OutputRuntimeState::Preparing => OutputState::Preparing,
        OutputRuntimeState::Ready => OutputState::Ready,
        OutputRuntimeState::Failed => OutputState::Failed,
        OutputRuntimeState::Closed => OutputState::Closed,
    }
}

impl StreamControlAdapter {
    pub fn new(identity: NodeIdentity, receive_endpoint: Endpoint) -> Self {
        Self {
            identity,
            receive_endpoint,
            streams: HashMap::new(),
            outputs: HashMap::new(),
            terminal_subscriptions: HashMap::new(),
            finalized_streams: HashMap::new(),
            restart_close_watches: HashMap::new(),
            media_tx: None,
            media_endpoints: None,
        }
    }

    pub fn with_media_endpoints(mut self, media_endpoints: Arc<MediaEndpointManager>) -> Self {
        self.media_endpoints = Some(media_endpoints);
        self
    }

    pub fn with_media_tx(mut self, media_tx: mpsc::Sender<u32>) -> Self {
        self.media_tx = Some(media_tx);
        self
    }

    fn should_attempt_output_creation(&self, stream_id: &str) -> bool {
        self.media_tx.is_some() || self.streams.contains_key(stream_id)
    }

    pub async fn start_receive(&mut self, request: StartReceiveRequest) -> StartReceiveResponse {
        let transport = MediaTransport::try_from(request.media_transport)
            .unwrap_or(MediaTransport::Unspecified);
        if transport == MediaTransport::Unspecified && request.media_transport != 0 {
            return start_response(
                &request.stream_id,
                StreamState::Failed,
                vec![],
                Some(error("invalid_media_transport", "unknown media transport")),
            );
        }
        let transport = if transport == MediaTransport::Unspecified {
            MediaTransport::Udp
        } else {
            transport
        };
        if !self.matches_expected(request.expected_stream.as_ref()) {
            return start_response(
                &request.stream_id,
                StreamState::Failed,
                vec![],
                Some(error("stale_instance", "stream instance does not match")),
            );
        }
        if request.lease_id.is_empty() || request.route_id.is_empty() {
            return start_response(
                &request.stream_id,
                StreamState::Failed,
                vec![],
                Some(error("invalid_lease", "lease_id and route_id are required")),
            );
        }
        if self.restart_close_watches.contains_key(&request.stream_id) {
            return start_response(
                &request.stream_id,
                StreamState::Stopping,
                vec![],
                Some(error("stream_closing", "stream is closing")),
            );
        }
        if let (Some(media_endpoints), Some(existing)) = (
            self.media_endpoints.as_ref(),
            self.streams.get(&request.stream_id),
        ) && existing.lease_id != request.lease_id
            && !media_endpoints
                .owns_active_lease(&request.stream_id, &existing.lease_id)
                .await
        {
            self.streams.remove(&request.stream_id);
        }
        if let Some(existing) = self.streams.get(&request.stream_id) {
            if existing.lease_id == request.lease_id {
                if self.media_endpoints.is_none() {
                    return start_response(
                        &request.stream_id,
                        existing.state,
                        existing.endpoints.clone(),
                        None,
                    );
                }
            } else {
                return start_response(
                    &request.stream_id,
                    StreamState::Failed,
                    vec![],
                    Some(error(
                        "idempotency_conflict",
                        "stream already has a different lease",
                    )),
                );
            }
        }
        self.finalized_streams.remove(&request.stream_id);
        let endpoints = if let Some(media_endpoints) = &self.media_endpoints {
            let expected_ssrc = request
                .constraints
                .get("expected_ssrc")
                .and_then(|value| value.parse::<u32>().ok());
            let reservation_ttl = (request.reservation_ttl_ms != 0)
                .then(|| Duration::from_millis(request.reservation_ttl_ms));
            match media_endpoints
                .reserve(ReserveMediaEndpoint {
                    stream_id: request.stream_id.clone(),
                    lease_id: request.lease_id.clone(),
                    route_id: request.route_id.clone(),
                    expected_ssrc,
                    reservation_ttl,
                    confirmed: false,
                })
                .await
            {
                Ok(endpoint) => {
                    let mut endpoint = endpoint.endpoint(&self.receive_endpoint.host);
                    endpoint.labels.insert(
                        "media_transport".to_string(),
                        media_transport_name(transport).to_string(),
                    );
                    endpoint.labels.insert(
                        "transport_state".to_string(),
                        if transport == MediaTransport::TcpActive {
                            "listening"
                        } else {
                            "ready"
                        }
                        .to_string(),
                    );
                    vec![endpoint]
                }
                Err(error_value) => {
                    return start_response(
                        &request.stream_id,
                        StreamState::Failed,
                        vec![],
                        Some(error(
                            "endpoint_allocation_failed",
                            &error_value.to_string(),
                        )),
                    );
                }
            }
        } else if request.preferred_endpoints.is_empty() {
            vec![self.receive_endpoint.clone()]
        } else {
            request.preferred_endpoints.clone()
        };
        self.streams.insert(
            request.stream_id.clone(),
            StreamRuntime {
                lease_id: request.lease_id,
                route_id: request.route_id,
                endpoints: endpoints.clone(),
                state: StreamState::Receiving,
                primary_output_format: String::new(),
            },
        );
        start_response(&request.stream_id, StreamState::Receiving, endpoints, None)
    }

    pub async fn configure_receive_transport(
        &self,
        request: ConfigureReceiveTransportRequest,
    ) -> ConfigureReceiveTransportResponse {
        let transport = MediaTransport::try_from(request.media_transport)
            .unwrap_or(MediaTransport::Unspecified);
        let local_endpoint = self.streams.get(&request.stream_id).and_then(|stream| {
            stream
                .endpoints
                .iter()
                .find(|endpoint| {
                    endpoint.labels.get("endpoint_id") == Some(&request.endpoint_id)
                        && endpoint
                            .labels
                            .get("generation")
                            .and_then(|value| value.parse::<u64>().ok())
                            == Some(request.endpoint_generation)
                })
                .cloned()
        });
        let Some(local_endpoint) = local_endpoint else {
            return configure_transport_response(
                MediaTransportState::Failed,
                None,
                request.remote_endpoint,
                Some(error(
                    "stale_endpoint_generation",
                    "receive endpoint is stale",
                )),
            );
        };
        if transport == MediaTransport::Udp || transport == MediaTransport::TcpPassive {
            return configure_transport_response(
                MediaTransportState::Ready,
                Some(local_endpoint),
                request.remote_endpoint,
                None,
            );
        }
        if transport != MediaTransport::TcpActive {
            return configure_transport_response(
                MediaTransportState::Failed,
                Some(local_endpoint),
                request.remote_endpoint,
                Some(error(
                    "invalid_media_transport",
                    "media transport is required",
                )),
            );
        }
        let Some(remote_endpoint) = request.remote_endpoint else {
            return configure_transport_response(
                MediaTransportState::Failed,
                Some(local_endpoint),
                None,
                Some(error(
                    "media_peer_policy_required",
                    "TCP active requires a remote media endpoint",
                )),
            );
        };
        let remote_addr = format!("{}:{}", remote_endpoint.host, remote_endpoint.port)
            .parse::<std::net::SocketAddr>();
        let Ok(remote_addr) = remote_addr else {
            return configure_transport_response(
                MediaTransportState::Failed,
                Some(local_endpoint),
                Some(remote_endpoint),
                Some(error(
                    "media_peer_policy_required",
                    "remote media endpoint must use an IP address and valid port",
                )),
            );
        };
        let Some(manager) = self.media_endpoints.as_ref() else {
            return configure_transport_response(
                MediaTransportState::Failed,
                Some(local_endpoint),
                Some(remote_endpoint),
                Some(error(
                    "media_transport_unsupported",
                    "managed media endpoints are unavailable",
                )),
            );
        };
        let timeout = Duration::from_millis(request.connect_timeout_ms.clamp(1, 30_000));
        match manager
            .connect_tcp_active(ConnectMediaEndpoint {
                stream_id: request.stream_id,
                lease_id: request.lease_id,
                route_id: request.route_id,
                endpoint_id: request.endpoint_id,
                generation: request.endpoint_generation,
                remote_addr,
                local_addr: None,
                timeout,
            })
            .await
        {
            Ok(MediaConnectionState::Ready) => configure_transport_response(
                MediaTransportState::Ready,
                Some(local_endpoint),
                Some(remote_endpoint),
                None,
            ),
            Ok(state) => configure_transport_response(
                media_connection_state(state),
                Some(local_endpoint),
                Some(remote_endpoint),
                None,
            ),
            Err(error_value) => configure_transport_response(
                MediaTransportState::Failed,
                Some(local_endpoint),
                Some(remote_endpoint),
                Some(error(
                    "stream_transport_not_ready",
                    &error_value.to_string(),
                )),
            ),
        }
    }

    pub async fn stop_receive(&mut self, request: StopReceiveRequest) -> StopReceiveResponse {
        let cutoff = now_ms().saturating_sub(10 * 60 * 1_000);
        self.finalized_streams
            .retain(|_, finalized| finalized.finalized_at_ms >= cutoff);
        if (!request.expected_lease_id.is_empty() || !request.expected_route_id.is_empty())
            && !self.streams.get(&request.stream_id).is_some_and(|stream| {
                (request.expected_lease_id.is_empty()
                    || stream.lease_id == request.expected_lease_id)
                    && (request.expected_route_id.is_empty()
                        || stream.route_id == request.expected_route_id)
            })
        {
            return stop_response(StreamState::Stopped, None, true, true, None);
        }
        let stream_id = request.stream_id.clone();
        let mut response = match StopReceivePhase::try_from(request.phase)
            .unwrap_or(StopReceivePhase::Unspecified)
        {
            StopReceivePhase::Unspecified => self.stop_receive_legacy(request),
            StopReceivePhase::QuiesceOutputs => self.quiesce_receive_outputs(request),
            StopReceivePhase::Finalize => self.finalize_receive(request),
        };
        if response.state == StreamState::Stopped as i32 {
            let lease_id = self
                .streams
                .get(&stream_id)
                .filter(|stream| stream.state == StreamState::Stopped)
                .map(|stream| stream.lease_id.clone());
            if let (Some(media_endpoints), Some(lease_id)) = (&self.media_endpoints, &lease_id)
                && let Err(error_value) = media_endpoints.release(&stream_id, lease_id).await
            {
                response.state = StreamState::Stopping as i32;
                response.error = Some(error("endpoint_release_failed", &error_value.to_string()));
                response.input_removed = false;
                return response;
            }
            if let Some(lease_id) = lease_id
                && self
                    .streams
                    .get(&stream_id)
                    .is_some_and(|stream| stream.lease_id == lease_id)
            {
                self.streams.remove(&stream_id);
            }
        }
        response
    }

    fn stop_receive_legacy(&mut self, request: StopReceiveRequest) -> StopReceiveResponse {
        if self.media_tx.is_some() {
            Register::close_stream_by_id(&request.stream_id);
        }
        self.outputs
            .retain(|_, output| output.stream_id != request.stream_id);
        self.restart_close_watches.remove(&request.stream_id);
        if let Some(stream) = self.streams.get_mut(&request.stream_id) {
            stream.state = StreamState::Stopped;
        }
        stop_response(StreamState::Stopped, None, true, true, None)
    }

    fn quiesce_receive_outputs(&mut self, request: StopReceiveRequest) -> StopReceiveResponse {
        let Some(expected_ssrc) = expected_ssrc(&request) else {
            return stop_response(
                StreamState::Failed,
                Some(error(
                    "invalid_stream_identity",
                    "expected_ssrc is required",
                )),
                false,
                false,
                None,
            );
        };
        if self.media_tx.is_none() {
            return stop_response(
                StreamState::Failed,
                Some(error(
                    "media_runtime_unavailable",
                    "stream media runtime is unavailable",
                )),
                false,
                false,
                None,
            );
        }
        if let Some(watch) = self.restart_close_watches.get(&request.stream_id)
            && watch.ssrc != expected_ssrc
        {
            return stop_response(
                StreamState::Stopping,
                Some(error(
                    "stream_generation_changed",
                    "restart close watch SSRC changed",
                )),
                true,
                false,
                self.restart_close_observation(&request.stream_id),
            );
        }
        let observation = match Register::stream_runtime_observation(&request.stream_id) {
            Some(current) => {
                if request.expected_lifecycle_generation != 0
                    && request.expected_lifecycle_generation != current.lifecycle_generation
                {
                    return stop_response(
                        StreamState::Failed,
                        Some(error(
                            "stream_generation_changed",
                            "stream lifecycle generation changed",
                        )),
                        false,
                        false,
                        Some(current),
                    );
                }
                match Register::quiesce_stream_outputs(
                    &request.stream_id,
                    expected_ssrc,
                    current.lifecycle_generation,
                ) {
                    Ok(observation) => observation,
                    Err(error_value) => {
                        return stop_response(
                            StreamState::Failed,
                            Some(detail_from_error(error_value)),
                            false,
                            false,
                            None,
                        );
                    }
                }
            }
            None => self.begin_restart_close_watch(&request.stream_id, expected_ssrc),
        };
        self.outputs
            .retain(|_, output| output.stream_id != request.stream_id);
        if let Some(stream) = self.streams.get_mut(&request.stream_id) {
            stream.state = StreamState::Stopping;
        }
        stop_response(StreamState::Stopping, None, true, false, Some(observation))
    }

    fn finalize_receive(&mut self, request: StopReceiveRequest) -> StopReceiveResponse {
        let Some((expected_ssrc, expected_generation)) = stop_identity(&request) else {
            return stop_response(
                StreamState::Failed,
                Some(error(
                    "invalid_stream_identity",
                    "expected_ssrc and expected_lifecycle_generation are required",
                )),
                false,
                false,
                None,
            );
        };
        if self
            .finalized_streams
            .get(&request.stream_id)
            .is_some_and(|finalized| {
                finalized.ssrc == expected_ssrc
                    && finalized.lifecycle_generation == expected_generation
                    && finalized.packet_count == request.expected_packet_count
            })
        {
            let finalized = self.finalized_streams[&request.stream_id].clone();
            return StopReceiveResponse {
                state: StreamState::Stopped as i32,
                error: None,
                outputs_closed: true,
                input_removed: true,
                ssrc: finalized.ssrc.to_string(),
                lifecycle_generation: finalized.lifecycle_generation,
                last_packet_at_ms: finalized.last_packet_at_ms,
                packet_count: finalized.packet_count,
                input_idle_timeout_ms: finalized.input_idle_timeout_ms,
            };
        }
        if let Some(watch) = self.restart_close_watches.get(&request.stream_id).cloned() {
            if watch.ssrc != expected_ssrc || watch.lifecycle_generation != expected_generation {
                return stop_response(
                    StreamState::Stopping,
                    Some(error(
                        "stream_generation_changed",
                        "restart close watch identity changed",
                    )),
                    true,
                    false,
                    self.restart_close_observation(&request.stream_id),
                );
            }
            let observation = self
                .restart_close_observation(&request.stream_id)
                .unwrap_or_else(|| restart_close_observation(&watch, None));
            if observation.packet_count != request.expected_packet_count {
                return stop_response(
                    StreamState::Stopping,
                    Some(error(
                        "stream_input_changed",
                        "stream input changed during finalize",
                    )),
                    true,
                    false,
                    Some(observation),
                );
            }
            self.restart_close_watches.remove(&request.stream_id);
            self.outputs
                .retain(|_, output| output.stream_id != request.stream_id);
            if let Some(stream) = self.streams.get_mut(&request.stream_id) {
                stream.state = StreamState::Stopped;
            }
            self.finalized_streams.insert(
                request.stream_id,
                FinalizedStreamRuntime {
                    ssrc: observation.ssrc,
                    lifecycle_generation: observation.lifecycle_generation,
                    last_packet_at_ms: observation.last_packet_at_ms,
                    packet_count: observation.packet_count,
                    input_idle_timeout_ms: observation.input_idle_timeout_ms,
                    finalized_at_ms: now_ms(),
                },
            );
            return stop_response(StreamState::Stopped, None, true, true, Some(observation));
        }
        let observation = if self.media_tx.is_some() {
            match Register::finalize_stream_by_id(
                &request.stream_id,
                expected_ssrc,
                expected_generation,
                request.expected_packet_count,
            ) {
                Ok(FinalizeStreamResult::Finalized(observation)) => observation,
                Ok(FinalizeStreamResult::InputChanged(observation)) => {
                    return stop_response(
                        StreamState::Stopping,
                        Some(error(
                            "stream_input_changed",
                            "stream input changed during finalize",
                        )),
                        true,
                        false,
                        Some(observation),
                    );
                }
                Err(error_value) => {
                    return stop_response(
                        StreamState::Stopping,
                        Some(detail_from_error(error_value)),
                        true,
                        false,
                        Register::stream_runtime_observation(&request.stream_id),
                    );
                }
            }
        } else {
            return stop_response(
                StreamState::Failed,
                Some(error(
                    "media_runtime_unavailable",
                    "stream media runtime is unavailable",
                )),
                false,
                false,
                None,
            );
        };
        self.outputs
            .retain(|_, output| output.stream_id != request.stream_id);
        if let Some(stream) = self.streams.get_mut(&request.stream_id) {
            stream.state = StreamState::Stopped;
        }
        self.finalized_streams.insert(
            request.stream_id,
            FinalizedStreamRuntime {
                ssrc: observation.ssrc,
                lifecycle_generation: observation.lifecycle_generation,
                last_packet_at_ms: observation.last_packet_at_ms,
                packet_count: observation.packet_count,
                input_idle_timeout_ms: observation.input_idle_timeout_ms,
                finalized_at_ms: now_ms(),
            },
        );
        stop_response(StreamState::Stopped, None, true, true, Some(observation))
    }

    pub fn query_stream(&self, request: QueryStreamRequest) -> QueryStreamResponse {
        let modern_state = self
            .streams
            .get(&request.stream_id)
            .map(|stream| stream.state);
        let observation = self
            .media_tx
            .as_ref()
            .and_then(|_| Register::stream_runtime_observation(&request.stream_id))
            .or_else(|| self.restart_close_observation(&request.stream_id));
        let legacy_register_ts = self.media_tx.as_ref().and_then(|_| {
            Register::get_base_stream_info_by_stream_id(Arc::from(request.stream_id.as_str()))
                .map(|info| info.in_time)
        });
        let state = if observation.is_some_and(|observation| observation.closing) {
            StreamState::Stopping
        } else {
            effective_stream_state(modern_state, legacy_register_ts)
        };
        let (viewer_count, viewer_formats) = self.viewer_stats(&request.stream_id);
        let primary_output_format = self
            .streams
            .get(&request.stream_id)
            .map(|stream| stream.primary_output_format.clone())
            .unwrap_or_default();
        let primary_output_metadata = self.media_tx.as_ref().and_then(|_| {
            Register::output_media_metadata(&request.stream_id, &primary_output_format)
        });
        let output_ready = if self.media_tx.is_some() {
            primary_output_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.state == OutputRuntimeState::Ready)
        } else {
            self.outputs.values().any(|output| {
                output.stream_id == request.stream_id && output.state == OutputState::Ready
            })
        };
        let actual_media_profile = self
            .media_tx
            .as_ref()
            .and_then(|_| Register::actual_media_profile(&request.stream_id));
        let readiness_stage = if state == StreamState::Failed
            || primary_output_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.state == OutputRuntimeState::Failed)
        {
            MediaReadinessStage::Failed
        } else if output_ready {
            MediaReadinessStage::OutputReady
        } else if actual_media_profile.is_some() {
            MediaReadinessStage::CodecReady
        } else if observation.is_some() {
            MediaReadinessStage::InputObserved
        } else {
            MediaReadinessStage::Starting
        };
        let media_ext = self
            .media_tx
            .as_ref()
            .and_then(|_| Register::stream_media_ext(&request.stream_id));
        let (video_width, video_height) = media_ext
            .as_ref()
            .and_then(|ext| ext.video_params.resolution)
            .map(|(width, height)| (width.max(0) as u32, height.max(0) as u32))
            .unwrap_or_default();
        QueryStreamResponse {
            stream_id: request.stream_id,
            state: state as i32,
            outputs: self.playback_endpoints(),
            playback_id: String::new(),
            playback_generation: 0,
            source_position_ms: 0,
            media_ready: readiness_stage == MediaReadinessStage::OutputReady,
            terminal_reason: String::new(),
            viewer_count,
            viewer_formats,
            ssrc: observation
                .map(|observation| observation.ssrc.to_string())
                .unwrap_or_default(),
            lifecycle_generation: observation
                .map(|observation| observation.lifecycle_generation)
                .unwrap_or_default(),
            last_packet_at_ms: observation
                .map(|observation| observation.last_packet_at_ms)
                .unwrap_or_default(),
            packet_count: observation
                .map(|observation| observation.packet_count)
                .unwrap_or_default(),
            input_idle_timeout_ms: observation
                .map(|observation| observation.input_idle_timeout_ms)
                .unwrap_or_default(),
            input_observed: observation.is_some(),
            primary_output_format,
            readiness_stage: readiness_stage as i32,
            video_codec: actual_media_profile
                .as_ref()
                .map(|profile| profile.video_codec.clone())
                .unwrap_or_default(),
            video_width,
            video_height,
            video_fps: media_ext
                .as_ref()
                .and_then(|ext| ext.video_params.fps)
                .unwrap_or_default()
                .max(0) as f64,
            input_bitrate_bps: media_ext
                .as_ref()
                .and_then(|ext| ext.video_params.bitrate)
                .unwrap_or_default()
                .max(0) as u64
                * 1_000,
            rtp_loss_count: 0,
            queue_drop_count: 0,
            audio_codec: actual_media_profile
                .map(|profile| profile.audio_codec)
                .unwrap_or_default(),
            mime_codec: primary_output_metadata
                .map(|metadata| metadata.mime_codec)
                .unwrap_or_default(),
        }
    }

    fn viewer_stats(&self, stream_id: &str) -> (u32, Vec<ViewerFormatCount>) {
        let mut viewers = HashSet::new();
        let mut formats = BTreeMap::<String, HashSet<String>>::new();
        for output in self.outputs.values().filter(|output| {
            output.stream_id == stream_id
                && matches!(output.state, OutputState::Preparing | OutputState::Ready)
                && !output.subscription_id.is_empty()
                && (self.media_tx.is_none()
                    || Register::is_live_output_open(stream_id, &output.output_type))
        }) {
            viewers.insert(output.subscription_id.clone());
            formats
                .entry(output.output_type.clone())
                .or_default()
                .insert(output.subscription_id.clone());
        }
        let viewer_count = u32::try_from(viewers.len()).unwrap_or(u32::MAX);
        let viewer_formats = formats
            .into_iter()
            .map(|(media_format, subscriptions)| ViewerFormatCount {
                media_format,
                viewer_count: u32::try_from(subscriptions.len()).unwrap_or(u32::MAX),
            })
            .collect();
        (viewer_count, viewer_formats)
    }

    fn begin_restart_close_watch(
        &mut self,
        stream_id: &str,
        expected_ssrc: u32,
    ) -> StreamRuntimeObservation {
        if let Some(existing) = self.restart_close_watches.get(stream_id) {
            if existing.ssrc == expected_ssrc {
                return restart_close_observation(
                    existing,
                    Register::unknown_stream_observation(expected_ssrc),
                );
            }
        }
        let started_at_ms = u64::try_from(now_ms()).unwrap_or(1).max(1);
        let watch = RestartCloseWatch {
            ssrc: expected_ssrc,
            lifecycle_generation: started_at_ms,
            started_at_ms,
            input_idle_timeout_ms: Register::configured_input_idle_timeout_ms().max(1),
        };
        let observation =
            restart_close_observation(&watch, Register::unknown_stream_observation(expected_ssrc));
        self.restart_close_watches
            .insert(stream_id.to_string(), watch);
        observation
    }

    fn restart_close_observation(&self, stream_id: &str) -> Option<StreamRuntimeObservation> {
        let watch = self.restart_close_watches.get(stream_id)?;
        Some(restart_close_observation(
            watch,
            Register::unknown_stream_observation(watch.ssrc),
        ))
    }

    pub fn create_output(&mut self, request: CreateOutputRequest) -> CreateOutputResponse {
        self.prune_terminal_subscriptions(now_ms());
        if self.streams.get(&request.stream_id).is_some_and(|stream| {
            matches!(stream.state, StreamState::Stopping | StreamState::Stopped)
        }) {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error("stream_closing", "stream is closing")),
                output: None,
            };
        }
        if !request.subscription_id.is_empty()
            && self
                .terminal_subscriptions
                .contains_key(&(request.stream_id.clone(), request.subscription_id.clone()))
        {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error(
                    "subscription_terminal",
                    "stream subscription is terminal",
                )),
                output: None,
            };
        }
        if request.endpoint_mode == EndpointMode::Multi as i32 {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error(
                    "multi_endpoint_disabled",
                    "multi RTP endpoint pool is reserved but not enabled",
                )),
                output: None,
            };
        }
        if !self.should_attempt_output_creation(&request.stream_id) {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error("stream_not_found", "stream is not receiving")),
                output: None,
            };
        }
        let output_type = match normalize_live_output_type(&request.output_type) {
            Some(output_type) => output_type,
            None => {
                return CreateOutputResponse {
                    output_id: String::new(),
                    endpoints: vec![],
                    error: Some(error(
                        "unsupported_output_type",
                        "output_type must be flv, fmp4, hls, or ll_hls",
                    )),
                    output: None,
                };
            }
        };
        let operation_id = request
            .operation
            .as_ref()
            .map(|operation| operation.idempotency_key.trim())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                request
                    .operation
                    .as_ref()
                    .map(|operation| operation.operation_id.trim())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or(&request.stream_id);
        let output_id = format!("out-{output_type}-{operation_id}");
        if let Some(mut existing) = self.outputs.get(&output_id).cloned() {
            if existing.subscription_id != request.subscription_id {
                return CreateOutputResponse {
                    output_id: String::new(),
                    endpoints: self.playback_endpoints(),
                    error: Some(error(
                        "idempotency_conflict",
                        "output operation belongs to another subscription",
                    )),
                    output: None,
                };
            }
            if self.media_tx.is_some() {
                match Register::create_live_output(
                    &request.stream_id,
                    output_type,
                    &request.audio_codec,
                ) {
                    Ok(endpoint) => {
                        existing.endpoint = endpoint;
                        self.outputs.insert(output_id.clone(), existing.clone());
                    }
                    Err(error_value) => {
                        return CreateOutputResponse {
                            output_id: String::new(),
                            endpoints: self.playback_endpoints(),
                            error: Some(detail_from_error(error_value)),
                            output: None,
                        };
                    }
                }
            }
            let output = existing.info(self.media_tx.is_some());
            return CreateOutputResponse {
                output_id,
                endpoints: self.playback_endpoints(),
                error: None,
                output: Some(output),
            };
        }
        let endpoint = if self.media_tx.is_some() {
            match Register::create_live_output(
                &request.stream_id,
                output_type,
                &request.audio_codec,
            ) {
                Ok(endpoint) => endpoint,
                Err(error_value) => {
                    return CreateOutputResponse {
                        output_id: String::new(),
                        endpoints: vec![],
                        error: Some(detail_from_error(error_value)),
                        output: None,
                    };
                }
            }
        } else {
            String::new()
        };
        let runtime = OutputRuntime {
            output_id: output_id.clone(),
            stream_id: request.stream_id,
            output_type: output_type.to_string(),
            endpoint,
            state: if self.media_tx.is_some() {
                OutputState::Preparing
            } else {
                OutputState::Ready
            },
            subscription_id: request.subscription_id,
        };
        let output = runtime.info(self.media_tx.is_some());
        self.outputs.insert(output_id.clone(), runtime);
        CreateOutputResponse {
            output_id,
            endpoints: self.playback_endpoints(),
            error: None,
            output: Some(output),
        }
    }

    pub fn close_output(&mut self, request: CloseOutputRequest) -> CloseOutputResponse {
        let Some(runtime) = self.outputs.get(&request.output_id).cloned() else {
            return CloseOutputResponse {
                closed: false,
                error: None,
                output: None,
            };
        };
        if !request.stream_id.is_empty() && request.stream_id != runtime.stream_id {
            return CloseOutputResponse {
                closed: false,
                error: Some(error(
                    "output_stream_mismatch",
                    "output does not belong to stream",
                )),
                output: Some(runtime.info(self.media_tx.is_some())),
            };
        }
        self.outputs.remove(&request.output_id);
        let still_referenced = self.outputs.values().any(|output| {
            output.stream_id == runtime.stream_id
                && output_resource_type(&output.output_type)
                    == output_resource_type(&runtime.output_type)
        });
        if !still_referenced
            && self.media_tx.is_some()
            && let Err(error_value) =
                Register::close_live_output(&runtime.stream_id, &runtime.output_type)
        {
            self.outputs
                .insert(runtime.output_id.clone(), runtime.clone());
            return CloseOutputResponse {
                closed: false,
                error: Some(detail_from_error(error_value)),
                output: Some(runtime.info(self.media_tx.is_some())),
            };
        }
        let mut output = runtime.info(self.media_tx.is_some());
        output.state = OutputState::Closed as i32;
        CloseOutputResponse {
            closed: true,
            error: None,
            output: Some(output),
        }
    }

    pub fn release_subscription_outputs(
        &mut self,
        request: ReleaseSubscriptionOutputsRequest,
    ) -> ReleaseSubscriptionOutputsResponse {
        if request.stream_id.is_empty() || request.subscription_id.is_empty() {
            return ReleaseSubscriptionOutputsResponse {
                closed_output_ids: vec![],
                error: Some(error(
                    "invalid_subscription",
                    "stream_id and subscription_id are required",
                )),
            };
        }
        let now_ms = now_ms();
        self.prune_terminal_subscriptions(now_ms);
        self.terminal_subscriptions.insert(
            (request.stream_id.clone(), request.subscription_id.clone()),
            now_ms.saturating_add(TERMINAL_SUBSCRIPTION_RETENTION_MS),
        );
        let output_ids = self
            .outputs
            .values()
            .filter(|output| {
                output.stream_id == request.stream_id
                    && output.subscription_id == request.subscription_id
            })
            .map(|output| output.output_id.clone())
            .collect::<Vec<_>>();
        let mut closed_output_ids = Vec::with_capacity(output_ids.len());
        for output_id in output_ids {
            let response = self.close_output(CloseOutputRequest {
                operation: request.operation.clone(),
                output_id: output_id.clone(),
                stream_id: request.stream_id.clone(),
            });
            if let Some(error) = response.error {
                return ReleaseSubscriptionOutputsResponse {
                    closed_output_ids,
                    error: Some(error),
                };
            }
            closed_output_ids.push(output_id);
        }
        ReleaseSubscriptionOutputsResponse {
            closed_output_ids,
            error: None,
        }
    }

    fn prune_terminal_subscriptions(&mut self, now_ms: i64) {
        self.terminal_subscriptions
            .retain(|_, expires_at_ms| *expires_at_ms > now_ms);
    }

    pub fn get_playback_endpoints(
        &self,
        request: GetPlaybackEndpointsRequest,
    ) -> GetPlaybackEndpointsResponse {
        GetPlaybackEndpointsResponse {
            endpoints: self.playback_endpoints(),
            outputs: self
                .outputs
                .values()
                .filter(|output| output.stream_id == request.stream_id)
                .filter(|output| {
                    self.media_tx.is_none()
                        || Register::is_live_output_open(&output.stream_id, &output.output_type)
                        || Register::output_media_metadata(&output.stream_id, &output.output_type)
                            .is_some_and(|metadata| metadata.state == OutputRuntimeState::Failed)
                })
                .map(|output| output.info(self.media_tx.is_some()))
                .collect(),
        }
    }

    pub fn init_media(&mut self, request: StreamJsonRequest) -> StreamUnitResponse {
        let media_tx = match self.media_tx.clone() {
            Some(media_tx) => media_tx,
            None => {
                return StreamUnitResponse {
                    error: Some(error(
                        "media_tx_missing",
                        "stream media tx is not initialized",
                    )),
                };
            }
        };
        let subscription_id = request.subscription_id;
        let result = decode_payload::<MediaConfig>(&request.payload_json).and_then(|value| {
            let stream_id = value.stream_id.clone();
            let output_type = live_output_type_from_kind(&value.output);
            let primary_output_format = primary_output_type_from_kind(&value.output);
            let audio_codec = value
                .transcode
                .as_ref()
                .and_then(|transcode| transcode.audio_codec)
                .map(|_| "aac")
                .unwrap_or("");
            let ssrc = Register::init_media(value).map_err(detail_from_error)?;
            if let (Some(stream), Some(output_format)) =
                (self.streams.get_mut(&stream_id), primary_output_format)
            {
                stream.primary_output_format = output_format.to_string();
            }
            if let Some(output_type) = output_type {
                let endpoint = Register::create_live_output(&stream_id, output_type, audio_codec)
                    .map_err(detail_from_error)?;
                let output_id = format!("out-{output_type}-primary-{stream_id}");
                self.outputs
                    .entry(output_id.clone())
                    .or_insert(OutputRuntime {
                        output_id,
                        stream_id,
                        output_type: output_type.to_string(),
                        endpoint,
                        state: OutputState::Preparing,
                        subscription_id,
                    });
            }
            media_tx.try_send(ssrc).map_err(|err| {
                error(
                    "media_tx_busy",
                    &format!("send media init event failed: {err}"),
                )
            })
        });
        stream_unit_response(result)
    }

    pub fn resource_snapshot(&self) -> NodeResourceSnapshot {
        NodeResourceSnapshot {
            full: true,
            resources: self
                .streams
                .iter()
                .map(|(stream_id, stream)| {
                    let mut labels = HashMap::from([
                        ("route_id".to_string(), stream.route_id.clone()),
                        ("lease_id".to_string(), stream.lease_id.clone()),
                    ]);
                    if let Some(endpoint) = stream
                        .endpoints
                        .iter()
                        .find(|endpoint| endpoint.name == "rtp" || endpoint.scheme == "rtp")
                    {
                        labels.insert("media_host".to_string(), endpoint.host.clone());
                        labels.insert("media_port".to_string(), endpoint.port.to_string());
                        if let Some(endpoint_id) = endpoint.labels.get("endpoint_id") {
                            labels.insert("endpoint_id".to_string(), endpoint_id.clone());
                        }
                        if let Some(generation) = endpoint.labels.get("generation") {
                            labels.insert("endpoint_generation".to_string(), generation.clone());
                        }
                    }
                    labels.insert(
                        "listener_state".to_string(),
                        match stream.state {
                            StreamState::Receiving => "listening",
                            StreamState::Stopping => "releasing",
                            StreamState::Stopped => "stopped",
                            StreamState::Failed => "failed",
                            _ => "binding",
                        }
                        .to_string(),
                    );
                    ResourceReport {
                        resource: Some(ResourceRef {
                            resource_id: stream_id.clone(),
                            resource_type: "stream".to_string(),
                        }),
                        state: match stream.state {
                            StreamState::Receiving => ResourceState::Running as i32,
                            StreamState::Stopping => ResourceState::Stopping as i32,
                            StreamState::Stopped => ResourceState::Stopped as i32,
                            StreamState::Failed => ResourceState::Failed as i32,
                            _ => ResourceState::Starting as i32,
                        },
                        labels,
                    }
                })
                .collect(),
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

    fn playback_endpoints(&self) -> Vec<Endpoint> {
        vec![self.receive_endpoint.clone()]
    }
}

fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ErrorDetail> {
    serde_json::from_slice(payload).map_err(|err| {
        error(
            "invalid_payload",
            &format!("decode stream control payload failed: {err}"),
        )
    })
}

fn json_response<T: Serialize>(value: &T) -> StreamJsonResponse {
    match serde_json::to_vec(value) {
        Ok(payload_json) => StreamJsonResponse {
            payload_json,
            error: None,
        },
        Err(err) => StreamJsonResponse {
            payload_json: vec![],
            error: Some(error(
                "encode_failed",
                &format!("encode stream control response failed: {err}"),
            )),
        },
    }
}

fn stream_unit_response(result: Result<(), ErrorDetail>) -> StreamUnitResponse {
    match result {
        Ok(()) => StreamUnitResponse { error: None },
        Err(error) => StreamUnitResponse { error: Some(error) },
    }
}

fn detail_from_error(error_value: GlobalError) -> ErrorDetail {
    gmv_nodec::error::global_error_detail("stream_control_failed", &error_value)
}

async fn record_info_response(request: StreamJsonRequest) -> StreamJsonResponse {
    let info = match decode_payload::<StreamInfoQo>(&request.payload_json) {
        Ok(info) => info,
        Err(error) => {
            return StreamJsonResponse {
                payload_json: vec![],
                error: Some(error),
            };
        }
    };
    match info.output_enum {
        OutputEnum::LocalMp4 => {
            let (tx, rx) = oneshot::channel();
            if Register::try_publish_mpsc::<Mp4OutputInnerEvent>(
                info.ssrc,
                Mp4OutputInnerEvent::StoreInfo(tx),
            )
            .is_ok()
            {
                match rx.await {
                    Ok(record) => json_response(&record),
                    Err(err) => StreamJsonResponse {
                        payload_json: vec![],
                        error: Some(error(
                            "record_info_closed",
                            &format!("record info response channel closed: {err}"),
                        )),
                    },
                }
            } else {
                StreamJsonResponse {
                    payload_json: vec![],
                    error: Some(error("record_not_found", "record output is not available")),
                }
            }
        }
        _ => StreamJsonResponse {
            payload_json: vec![],
            error: Some(error("record_not_found", "record output is not available")),
        },
    }
}

fn close_output_by_ssrc(info: StreamInfoQo) -> Result<(), ErrorDetail> {
    match info.output_enum {
        OutputEnum::LocalMp4 => {
            Register::try_publish_mpsc::<Mp4OutputInnerEvent>(info.ssrc, Mp4OutputInnerEvent::Close)
                .map_err(detail_from_error)
        }
        _ => Ok(()),
    }
}

fn endpoint(name: &str, scheme: &str, host: &str, port: u32) -> Endpoint {
    Endpoint {
        name: name.to_string(),
        scheme: scheme.to_string(),
        host: host.to_string(),
        port,
        mode: EndpointMode::Single as i32,
        labels: HashMap::new(),
    }
}

fn start_response(
    stream_id: &str,
    state: StreamState,
    endpoints: Vec<Endpoint>,
    error: Option<ErrorDetail>,
) -> StartReceiveResponse {
    StartReceiveResponse {
        stream_id: stream_id.to_string(),
        state: state as i32,
        receive_endpoints: endpoints,
        error,
    }
}

fn configure_transport_response(
    state: MediaTransportState,
    local_endpoint: Option<Endpoint>,
    remote_endpoint: Option<Endpoint>,
    error: Option<ErrorDetail>,
) -> ConfigureReceiveTransportResponse {
    ConfigureReceiveTransportResponse {
        state: state as i32,
        local_endpoint,
        remote_endpoint,
        error,
    }
}

fn media_transport_name(transport: MediaTransport) -> &'static str {
    match transport {
        MediaTransport::Udp => "udp",
        MediaTransport::TcpActive => "tcp_active",
        MediaTransport::TcpPassive => "tcp_passive",
        MediaTransport::Unspecified => "udp",
    }
}

fn media_connection_state(state: MediaConnectionState) -> MediaTransportState {
    match state {
        MediaConnectionState::Listening => MediaTransportState::Listening,
        MediaConnectionState::Connecting => MediaTransportState::Connecting,
        MediaConnectionState::Ready => MediaTransportState::Ready,
        MediaConnectionState::Failed => MediaTransportState::Failed,
    }
}

fn expected_ssrc(request: &StopReceiveRequest) -> Option<u32> {
    let ssrc = request.expected_ssrc.trim().parse::<u32>().ok()?;
    (ssrc != 0).then_some(ssrc)
}

fn stop_identity(request: &StopReceiveRequest) -> Option<(u32, u64)> {
    expected_ssrc(request).and_then(|ssrc| {
        (request.expected_lifecycle_generation != 0)
            .then_some((ssrc, request.expected_lifecycle_generation))
    })
}

fn stop_response(
    state: StreamState,
    error: Option<ErrorDetail>,
    outputs_closed: bool,
    input_removed: bool,
    observation: Option<StreamRuntimeObservation>,
) -> StopReceiveResponse {
    StopReceiveResponse {
        state: state as i32,
        error,
        outputs_closed,
        input_removed,
        ssrc: observation
            .map(|observation| observation.ssrc.to_string())
            .unwrap_or_default(),
        lifecycle_generation: observation
            .map(|observation| observation.lifecycle_generation)
            .unwrap_or_default(),
        last_packet_at_ms: observation
            .map(|observation| observation.last_packet_at_ms)
            .unwrap_or_default(),
        packet_count: observation
            .map(|observation| observation.packet_count)
            .unwrap_or_default(),
        input_idle_timeout_ms: observation
            .map(|observation| observation.input_idle_timeout_ms)
            .unwrap_or_default(),
    }
}

fn error(code: &str, message: &str) -> ErrorDetail {
    gmv_nodec::error::error_detail(code, message)
}

fn normalize_live_output_type(output_type: &str) -> Option<&'static str> {
    match output_type.trim().to_ascii_lowercase().as_str() {
        "flv" | "http_flv" => Some("flv"),
        "fmp4" | "dash_fmp4" => Some("fmp4"),
        "hls" | "hls_fmp4" => Some("hls"),
        "ll_hls" => Some("ll_hls"),
        _ => None,
    }
}

fn live_output_type_from_kind(output: &OutputKind) -> Option<&'static str> {
    match output {
        OutputKind::HttpFlv(_) => Some("flv"),
        OutputKind::DashFmp4(_) => Some("fmp4"),
        OutputKind::HlsFmp4(output) => Some(match output.playlist_profile {
            gmv_domain::info::output::HlsPlaylistProfile::Standard => "hls",
            gmv_domain::info::output::HlsPlaylistProfile::LowLatency => "ll_hls",
        }),
        _ => None,
    }
}

fn primary_output_type_from_kind(output: &OutputKind) -> Option<&'static str> {
    match output {
        OutputKind::LocalMp4(_) => Some("mp4"),
        _ => live_output_type_from_kind(output),
    }
}

fn output_resource_type(output_type: &str) -> &str {
    match output_type {
        "hls" | "ll_hls" => "hls",
        value => value,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn restart_close_observation(
    watch: &RestartCloseWatch,
    unknown_observation: Option<(u64, u64)>,
) -> StreamRuntimeObservation {
    let (last_packet_at_ms, packet_count) = unknown_observation.unwrap_or((watch.started_at_ms, 0));
    StreamRuntimeObservation {
        ssrc: watch.ssrc,
        lifecycle_generation: watch.lifecycle_generation,
        last_packet_at_ms: last_packet_at_ms.max(watch.started_at_ms),
        packet_count,
        input_idle_timeout_ms: watch.input_idle_timeout_ms,
        closing: true,
    }
}

fn effective_stream_state(
    modern_state: Option<StreamState>,
    legacy_register_ts: Option<u64>,
) -> StreamState {
    match modern_state {
        Some(state) if state != StreamState::Stopped => state,
        _ => match legacy_register_ts {
            Some(0) => StreamState::Starting,
            Some(_) => StreamState::Receiving,
            None => StreamState::Stopped,
        },
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

    #[test]
    fn actual_output_runtime_state_maps_to_protocol_state() {
        assert_eq!(
            output_state(OutputRuntimeState::Preparing),
            OutputState::Preparing
        );
        assert_eq!(output_state(OutputRuntimeState::Ready), OutputState::Ready);
        assert_eq!(
            output_state(OutputRuntimeState::Failed),
            OutputState::Failed
        );
        assert_eq!(
            output_state(OutputRuntimeState::Closed),
            OutputState::Closed
        );
    }
    use crate::general::cfg::{MediaListenerConf, MediaListenerMode, MediaPortRange};
    use crate::io::media_endpoint::{MediaBootstrap, MediaEndpointManager, find_free_test_range};
    use base::utils::rt::GlobalRuntime;
    use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};

    #[test]
    fn local_mp4_is_reported_as_primary_output_format() {
        let output = OutputKind::LocalMp4(gmv_domain::info::output::LocalMp4Output {
            fmt: gmv_domain::info::format::Mp4::default(),
            path: String::new(),
            token: None,
            file_name: None,
            min_free_bytes: 0,
        });
        assert_eq!(primary_output_type_from_kind(&output), Some("mp4"));
    }

    #[tokio::test]
    async fn start_and_stop_receive_manage_a_concrete_dynamic_endpoint() {
        let port = find_free_test_range(1).start;
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            MediaListenerConf {
                mode: MediaListenerMode::Multi,
                bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                advertised_host: "127.0.0.1".to_string(),
                single_port: 0,
                port_range: MediaPortRange {
                    start: port,
                    end: port,
                },
                reservation_timeout_secs: 30,
            },
            MediaBootstrap::Multi,
        )
        .unwrap();
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            u32::from(port),
        );
        let mut control =
            StreamControlAdapter::new(node.identity.clone(), manager.capability_endpoint())
                .with_media_endpoints(manager.clone());
        let request = StartReceiveRequest {
            operation: Some(operation("start-dynamic")),
            stream_id: "stream-dynamic".to_string(),
            route_id: "route-dynamic".to_string(),
            lease_id: "lease-dynamic".to_string(),
            expected_stream: Some(node.identity.clone()),
            preferred_endpoints: vec![],
            constraints: HashMap::from([("expected_ssrc".to_string(), "1001".to_string())]),
            reservation_ttl_ms: 30_000,
            media_transport: MediaTransport::Udp as i32,
        };

        let first = control.start_receive(request.clone()).await;
        let repeated = control.start_receive(request).await;
        assert_eq!(first.state, StreamState::Receiving as i32);
        assert_eq!(first.receive_endpoints, repeated.receive_endpoints);
        assert_eq!(first.receive_endpoints[0].port, u32::from(port));
        assert_eq!(first.receive_endpoints[0].mode, EndpointMode::Single as i32);
        assert!(
            first.receive_endpoints[0]
                .labels
                .contains_key("endpoint_id")
        );
        assert!(first.receive_endpoints[0].labels.contains_key("generation"));

        let stopped = control
            .stop_receive(StopReceiveRequest {
                operation: Some(operation("stop-dynamic")),
                stream_id: "stream-dynamic".to_string(),
                reason: "test".to_string(),
                phase: StopReceivePhase::Unspecified as i32,
                expected_ssrc: String::new(),
                expected_lifecycle_generation: 0,
                expected_packet_count: 0,
                expected_lease_id: "lease-dynamic".to_string(),
                expected_route_id: "route-dynamic".to_string(),
            })
            .await;
        assert_eq!(stopped.state, StreamState::Stopped as i32);
        let rebound_tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let rebound_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        drop((rebound_tcp, rebound_udp));

        let restarted = control
            .start_receive(StartReceiveRequest {
                operation: Some(operation("restart-dynamic")),
                stream_id: "stream-dynamic".to_string(),
                route_id: "route-restarted".to_string(),
                lease_id: "lease-restarted".to_string(),
                expected_stream: Some(node.identity.clone()),
                preferred_endpoints: vec![],
                constraints: HashMap::from([("expected_ssrc".to_string(), "1001".to_string())]),
                reservation_ttl_ms: 30_000,
                media_transport: MediaTransport::Udp as i32,
            })
            .await;
        assert_eq!(restarted.state, StreamState::Receiving as i32);

        let late_old_release = control
            .stop_receive(StopReceiveRequest {
                operation: Some(operation("late-old-release")),
                stream_id: "stream-dynamic".to_string(),
                reason: "late_old_lease".to_string(),
                phase: StopReceivePhase::Unspecified as i32,
                expected_ssrc: String::new(),
                expected_lifecycle_generation: 0,
                expected_packet_count: 0,
                expected_lease_id: "lease-dynamic".to_string(),
                expected_route_id: "route-dynamic".to_string(),
            })
            .await;
        assert_eq!(late_old_release.state, StreamState::Stopped as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 1);
        assert_eq!(
            control.resource_snapshot().resources[0]
                .labels
                .get("lease_id")
                .map(String::as_str),
            Some("lease-restarted")
        );

        control.finalized_streams.insert(
            "stream-dynamic".to_string(),
            FinalizedStreamRuntime {
                ssrc: 1001,
                lifecycle_generation: 7,
                last_packet_at_ms: 10,
                packet_count: 20,
                input_idle_timeout_ms: 4_000,
                finalized_at_ms: now_ms(),
            },
        );
        let late_old_finalize = control
            .stop_receive(StopReceiveRequest {
                operation: Some(operation("late-old-finalize")),
                stream_id: "stream-dynamic".to_string(),
                reason: "late_old_generation".to_string(),
                phase: StopReceivePhase::Finalize as i32,
                expected_ssrc: "1001".to_string(),
                expected_lifecycle_generation: 7,
                expected_packet_count: 20,
                expected_lease_id: String::new(),
                expected_route_id: String::new(),
            })
            .await;
        assert_eq!(late_old_finalize.state, StreamState::Stopped as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 1);
        assert_eq!(
            control.resource_snapshot().resources[0]
                .labels
                .get("lease_id")
                .map(String::as_str),
            Some("lease-restarted")
        );
    }

    #[tokio::test]
    async fn expired_dynamic_reservation_can_be_reallocated_with_a_new_lease() {
        let port = find_free_test_range(1).start;
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            MediaListenerConf {
                mode: MediaListenerMode::Multi,
                bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                advertised_host: "127.0.0.1".to_string(),
                single_port: 0,
                port_range: MediaPortRange {
                    start: port,
                    end: port,
                },
                reservation_timeout_secs: 30,
            },
            MediaBootstrap::Multi,
        )
        .unwrap();
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            u32::from(port),
        );
        let mut control =
            StreamControlAdapter::new(node.identity.clone(), manager.capability_endpoint())
                .with_media_endpoints(manager.clone());
        let request = |lease_id: &str, route_id: &str| StartReceiveRequest {
            operation: Some(operation(lease_id)),
            stream_id: "stream-expired".to_string(),
            route_id: route_id.to_string(),
            lease_id: lease_id.to_string(),
            expected_stream: Some(node.identity.clone()),
            preferred_endpoints: vec![],
            constraints: HashMap::from([("expected_ssrc".to_string(), "1001".to_string())]),
            reservation_ttl_ms: 1,
            media_transport: MediaTransport::Udp as i32,
        };

        assert_eq!(
            control
                .start_receive(request("lease-old", "route-old"))
                .await
                .state,
            StreamState::Receiving as i32
        );
        base::tokio::time::sleep(Duration::from_millis(2)).await;
        manager.expire_once().await;

        assert_eq!(
            control
                .start_receive(request("lease-new", "route-new"))
                .await
                .state,
            StreamState::Receiving as i32
        );
        assert_eq!(
            control.resource_snapshot().resources[0]
                .labels
                .get("lease_id")
                .map(String::as_str),
            Some("lease-new")
        );
    }

    #[test]
    fn restart_close_watch_uses_acceptance_time_until_new_packets_arrive() {
        let watch = RestartCloseWatch {
            ssrc: 200_000_001,
            lifecycle_generation: 7,
            started_at_ms: 1_000,
            input_idle_timeout_ms: 4_000,
        };

        let initial = restart_close_observation(&watch, None);
        assert_eq!(initial.last_packet_at_ms, 1_000);
        assert_eq!(initial.packet_count, 0);
        assert!(initial.closing);

        let late = restart_close_observation(&watch, Some((1_500, 3)));
        assert_eq!(late.last_packet_at_ms, 1_500);
        assert_eq!(late.packet_count, 3);

        let stale = restart_close_observation(&watch, Some((500, 2)));
        assert_eq!(stale.last_packet_at_ms, 1_000);
        assert_eq!(stale.packet_count, 2);
    }

    #[tokio::test]
    async fn restart_close_watch_fences_a_new_receive_with_the_same_stream_id() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let mut control = StreamControlAdapter::new(
            node.identity.clone(),
            endpoint("rtp", "rtp", "127.0.0.1", 30000),
        );
        control.restart_close_watches.insert(
            "stream-a".to_string(),
            RestartCloseWatch {
                ssrc: 200_000_001,
                lifecycle_generation: 7,
                started_at_ms: 1_000,
                input_idle_timeout_ms: 4_000,
            },
        );

        let response = control
            .start_receive(StartReceiveRequest {
                operation: Some(operation("restart-stream-a")),
                stream_id: "stream-a".to_string(),
                route_id: "route-a".to_string(),
                lease_id: "lease-a".to_string(),
                expected_stream: Some(node.identity),
                preferred_endpoints: vec![],
                constraints: HashMap::new(),
                reservation_ttl_ms: 0,
                media_transport: MediaTransport::Udp as i32,
            })
            .await;

        assert_eq!(response.state, StreamState::Stopping as i32);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("stream_closing")
        );
        assert!(!control.streams.contains_key("stream-a"));
    }

    #[test]
    fn legacy_media_runtime_drives_stream_state() {
        assert_eq!(effective_stream_state(None, Some(0)), StreamState::Starting);
        assert_eq!(
            effective_stream_state(None, Some(1)),
            StreamState::Receiving
        );
        assert_eq!(
            effective_stream_state(Some(StreamState::Stopped), Some(1)),
            StreamState::Receiving
        );
        assert_eq!(effective_stream_state(None, None), StreamState::Stopped);
    }

    #[tokio::test]
    async fn stop_receive_removes_outputs_without_modern_stream_runtime() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let mut control =
            StreamControlAdapter::new(node.identity, endpoint("rtp", "rtp", "127.0.0.1", 30000));
        control.outputs.insert(
            "output-1".to_string(),
            OutputRuntime {
                output_id: "output-1".to_string(),
                stream_id: "legacy-stream".to_string(),
                output_type: "hls".to_string(),
                endpoint: String::new(),
                state: OutputState::Ready,
                subscription_id: "subscription-1".to_string(),
            },
        );

        let response = control
            .stop_receive(StopReceiveRequest {
                operation: None,
                stream_id: "legacy-stream".to_string(),
                reason: "manual_stop".to_string(),
                phase: StopReceivePhase::Unspecified as i32,
                expected_ssrc: String::new(),
                expected_lifecycle_generation: 0,
                expected_packet_count: 0,
                expected_lease_id: String::new(),
                expected_route_id: String::new(),
            })
            .await;

        assert_eq!(response.state, StreamState::Stopped as i32);
        assert!(control.outputs.is_empty());
    }

    #[test]
    fn data_plane_output_creation_defers_stream_existence_to_register() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let control =
            StreamControlAdapter::new(node.identity, endpoint("rtp", "rtp", "127.0.0.1", 30000));
        assert!(!control.should_attempt_output_creation("stream-from-init-media"));

        let (media_tx, _media_rx) = mpsc::channel(1);
        let control = control.with_media_tx(media_tx);
        assert!(control.should_attempt_output_creation("stream-from-init-media"));
        assert!(control.streams.is_empty());
    }

    #[tokio::test]
    async fn same_input_supports_four_formats_and_multi_user_independent_close() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let mut control = StreamControlAdapter::new(
            node.identity.clone(),
            endpoint("rtp", "rtp", "127.0.0.1", 30000),
        );
        let started = control
            .start_receive(StartReceiveRequest {
                operation: Some(operation("start-live")),
                stream_id: "stream-a".to_string(),
                route_id: "route-a".to_string(),
                lease_id: "lease-a".to_string(),
                expected_stream: Some(node.identity),
                preferred_endpoints: vec![],
                constraints: HashMap::new(),
                reservation_ttl_ms: 0,
                media_transport: MediaTransport::Udp as i32,
            })
            .await;
        assert_eq!(started.state, StreamState::Receiving as i32);

        let mut output_ids = HashMap::new();
        for output_type in ["flv", "hls", "ll_hls", "fmp4"] {
            let output = control.create_output(CreateOutputRequest {
                operation: Some(operation(&format!("create-{output_type}"))),
                stream_id: "stream-a".to_string(),
                output_type: output_type.to_string(),
                endpoint_mode: EndpointMode::Single as i32,
                audio_codec: "aac".to_string(),
                subscription_id: "subscription-1".to_string(),
            });
            assert!(output.error.is_none(), "failed to create {output_type}");
            output_ids.insert(output_type, output.output_id);
        }
        assert_eq!(control.outputs.len(), 4);

        let second_hls = control.create_output(CreateOutputRequest {
            operation: Some(operation("create-hls-user-2")),
            stream_id: "stream-a".to_string(),
            output_type: "hls".to_string(),
            endpoint_mode: EndpointMode::Single as i32,
            audio_codec: "aac".to_string(),
            subscription_id: "subscription-2".to_string(),
        });
        assert!(second_hls.error.is_none());
        assert_ne!(second_hls.output_id, output_ids["hls"]);
        assert_eq!(control.outputs.len(), 5);
        let monitored = control.query_stream(QueryStreamRequest {
            stream_id: "stream-a".to_string(),
        });
        assert_eq!(monitored.viewer_count, 2);
        assert_eq!(
            monitored
                .viewer_formats
                .iter()
                .map(|item| (item.media_format.as_str(), item.viewer_count))
                .collect::<Vec<_>>(),
            vec![("flv", 1), ("fmp4", 1), ("hls", 2), ("ll_hls", 1)]
        );

        let closed = control.close_output(CloseOutputRequest {
            operation: Some(operation("close-hls")),
            output_id: output_ids["hls"].clone(),
            stream_id: "stream-a".to_string(),
        });
        assert!(closed.closed);
        assert_eq!(control.outputs.len(), 4);
        assert!(
            control
                .outputs
                .values()
                .any(|output| output.output_type == "hls")
        );

        let closed = control.close_output(CloseOutputRequest {
            operation: Some(operation("close-hls-user-2")),
            output_id: second_hls.output_id,
            stream_id: "stream-a".to_string(),
        });
        assert!(closed.closed);
        assert_eq!(control.outputs.len(), 3);
        assert!(
            control
                .outputs
                .values()
                .any(|output| output.output_type == "ll_hls")
        );
        assert!(
            control
                .outputs
                .values()
                .any(|output| output.output_type == "flv")
        );
        assert!(
            control
                .outputs
                .values()
                .any(|output| output.output_type == "fmp4")
        );
        assert_eq!(output_resource_type("hls"), output_resource_type("ll_hls"));
    }

    #[tokio::test]
    async fn subscription_release_is_scoped_and_blocks_late_output_creation() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let mut control = StreamControlAdapter::new(
            node.identity.clone(),
            endpoint("rtp", "rtp", "127.0.0.1", 30000),
        );
        control
            .start_receive(StartReceiveRequest {
                operation: Some(operation("start-live")),
                stream_id: "stream-a".to_string(),
                route_id: "route-a".to_string(),
                lease_id: "lease-a".to_string(),
                expected_stream: Some(node.identity),
                preferred_endpoints: vec![],
                constraints: HashMap::new(),
                reservation_ttl_ms: 0,
                media_transport: MediaTransport::Udp as i32,
            })
            .await;
        for (operation_id, subscription_id) in [
            ("create-user-1", "subscription-1"),
            ("create-user-2", "subscription-2"),
        ] {
            let output = control.create_output(CreateOutputRequest {
                operation: Some(operation(operation_id)),
                stream_id: "stream-a".to_string(),
                output_type: "hls".to_string(),
                endpoint_mode: EndpointMode::Single as i32,
                audio_codec: "aac".to_string(),
                subscription_id: subscription_id.to_string(),
            });
            assert!(output.error.is_none());
        }

        let released = control.release_subscription_outputs(ReleaseSubscriptionOutputsRequest {
            operation: Some(operation("release-user-1")),
            stream_id: "stream-a".to_string(),
            subscription_id: "subscription-1".to_string(),
        });
        assert!(released.error.is_none());
        assert_eq!(released.closed_output_ids.len(), 1);
        assert_eq!(control.outputs.len(), 1);
        assert_eq!(
            control.outputs.values().next().unwrap().subscription_id,
            "subscription-2"
        );
        let repeated = control.release_subscription_outputs(ReleaseSubscriptionOutputsRequest {
            operation: Some(operation("release-user-1-retry")),
            stream_id: "stream-a".to_string(),
            subscription_id: "subscription-1".to_string(),
        });
        assert!(repeated.error.is_none());
        assert!(repeated.closed_output_ids.is_empty());

        let late = control.create_output(CreateOutputRequest {
            operation: Some(operation("late-user-1")),
            stream_id: "stream-a".to_string(),
            output_type: "flv".to_string(),
            endpoint_mode: EndpointMode::Single as i32,
            audio_codec: "aac".to_string(),
            subscription_id: "subscription-1".to_string(),
        });
        assert_eq!(
            late.error.as_ref().map(|error| error.code.as_str()),
            Some("subscription_terminal")
        );
    }

    #[tokio::test]
    async fn stream_registers_heartbeats_starts_idempotently_and_snapshots() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let register = node.register_request(NodeResourceSnapshot {
            resources: vec![],
            full: true,
        });
        assert_eq!(register.identity.unwrap().kind, NodeKind::Stream as i32);
        assert!(register.capabilities.contains(&"live".to_string()));
        assert_eq!(register.endpoints.len(), 2);

        let heartbeat = node.heartbeat_message(1, 1000, 0);
        assert!(matches!(
            heartbeat.payload,
            Some(node_to_guard_message::Payload::Heartbeat(_))
        ));

        let mut control = StreamControlAdapter::new(
            node.identity.clone(),
            endpoint("rtp", "rtp", "127.0.0.1", 30000),
        );
        let request = StartReceiveRequest {
            operation: Some(operation("op-1")),
            stream_id: "stream-a".to_string(),
            route_id: "route-a".to_string(),
            lease_id: "lease-a".to_string(),
            expected_stream: Some(node.identity.clone()),
            preferred_endpoints: vec![],
            constraints: HashMap::new(),
            reservation_ttl_ms: 0,
            media_transport: MediaTransport::Udp as i32,
        };
        assert_eq!(
            control.start_receive(request.clone()).await.state,
            StreamState::Receiving as i32
        );
        assert_eq!(
            control.start_receive(request).await.state,
            StreamState::Receiving as i32
        );
        let snapshot = control.resource_snapshot();
        assert_eq!(snapshot.resources.len(), 1);
        assert_eq!(
            snapshot.resources[0]
                .labels
                .get("media_port")
                .map(String::as_str),
            Some("30000")
        );
        assert_eq!(
            snapshot.resources[0]
                .labels
                .get("listener_state")
                .map(String::as_str),
            Some("listening")
        );
        let output = control.create_output(CreateOutputRequest {
            operation: Some(operation("out-1")),
            stream_id: "stream-a".to_string(),
            output_type: "flv".to_string(),
            endpoint_mode: EndpointMode::Single as i32,
            audio_codec: "aac".to_string(),
            subscription_id: "subscription-1".to_string(),
        });
        assert!(output.error.is_none());
        assert_eq!(
            control
                .get_playback_endpoints(GetPlaybackEndpointsRequest {
                    stream_id: "stream-a".to_string(),
                })
                .outputs
                .len(),
            1
        );
        assert!(
            control
                .close_output(CloseOutputRequest {
                    operation: Some(operation("close-out-1")),
                    output_id: output.output_id,
                    stream_id: "stream-a".to_string(),
                })
                .closed
        );
        assert!(
            control
                .create_output(CreateOutputRequest {
                    operation: Some(operation("out-2")),
                    stream_id: "stream-a".to_string(),
                    output_type: "rtp".to_string(),
                    endpoint_mode: EndpointMode::Multi as i32,
                    audio_codec: "aac".to_string(),
                    subscription_id: "subscription-1".to_string(),
                })
                .error
                .is_some()
        );

        let event = node.frame_ready_event(2, 1001, "stream-a", "frame-1", 500);
        assert!(matches!(
            event.payload,
            Some(node_to_guard_message::Payload::Event(_))
        ));
    }

    #[tokio::test]
    async fn stream_rejects_stale_instance_without_touching_existing_state() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
            false,
            30000,
        );
        let mut control = StreamControlAdapter::new(
            node.identity.clone(),
            endpoint("rtp", "rtp", "127.0.0.1", 30000),
        );
        let stale = NodeIdentity {
            node_id: "stream-1".to_string(),
            instance_id: "old".to_string(),
            kind: NodeKind::Stream as i32,
        };
        let response = control
            .start_receive(StartReceiveRequest {
                operation: Some(operation("op-stale")),
                stream_id: "stream-a".to_string(),
                route_id: "route-a".to_string(),
                lease_id: "lease-a".to_string(),
                expected_stream: Some(stale),
                preferred_endpoints: vec![],
                constraints: HashMap::new(),
                reservation_ttl_ms: 0,
                media_transport: MediaTransport::Udp as i32,
            })
            .await;
        assert_eq!(response.state, StreamState::Failed as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 0);
    }
}
