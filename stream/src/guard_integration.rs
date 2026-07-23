use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::serde::{Serialize, de::DeserializeOwned};
use base::serde_json;
use base::tokio::sync::{mpsc, oneshot};
use base_rpc::RpcChannelConfig;
use gmv_domain::info::media_info::MediaConfig;
use gmv_domain::info::media_info_ext::MediaMap;
use gmv_domain::info::obj::{
    StreamInfoQo, StreamKey, StreamRecordInfo, TalkAnswerReq, TalkCloseReq, TalkOpenReq,
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
    CloseOutputRequest, CloseOutputResponse, CreateOutputRequest, CreateOutputResponse,
    GetPlaybackEndpointsRequest, GetPlaybackEndpointsResponse, OutputInfo, OutputState,
    QueryStreamRequest, QueryStreamResponse, ReleaseSubscriptionOutputsRequest,
    ReleaseSubscriptionOutputsResponse, StartReceiveRequest, StartReceiveResponse,
    StopReceiveRequest, StopReceiveResponse, StreamBoolResponse, StreamJsonRequest,
    StreamJsonResponse, StreamState, StreamUnitResponse, ViewerFormatCount,
    stream_control_server::StreamControl,
};
use tonic::transport::Channel;

use crate::io::local::mp4::Mp4OutputInnerEvent;
use crate::io::talk::TalkManager;
use crate::state::register::Register;

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
                "talk".to_string(),
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
}

#[tonic::async_trait]
impl StreamControl for StreamControlRpc {
    async fn start_receive(
        &self,
        request: tonic::Request<StartReceiveRequest>,
    ) -> Result<tonic::Response<StartReceiveResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.start_receive, req:{request:?}");
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(control.start_receive(request)))
    }

    async fn stop_receive(
        &self,
        request: tonic::Request<StopReceiveRequest>,
    ) -> Result<tonic::Response<StopReceiveResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.stop_receive, req:{request:?}");
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(control.stop_receive(request)))
    }

    async fn query_stream(
        &self,
        request: tonic::Request<QueryStreamRequest>,
    ) -> Result<tonic::Response<QueryStreamResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.query_stream, req:{request:?}");
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(control.query_stream(request)))
    }

    async fn create_output(
        &self,
        request: tonic::Request<CreateOutputRequest>,
    ) -> Result<tonic::Response<CreateOutputResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.create_output, req:{request:?}");
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(control.create_output(request)))
    }

    async fn close_output(
        &self,
        request: tonic::Request<CloseOutputRequest>,
    ) -> Result<tonic::Response<CloseOutputResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("stream_control.close_output, req:{request:?}");
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
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
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
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
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
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
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
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

    async fn talk_open(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.talk_open, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(
            match decode_payload::<TalkOpenReq>(&request.payload_json) {
                Ok(value) => match TalkManager::open(value).await {
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

    async fn talk_answer(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.talk_answer, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<TalkAnswerReq>(&request.payload_json)
                .and_then(|value| TalkManager::answer(value).map_err(detail_from_error)),
        )))
    }

    async fn talk_close(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.talk_close, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<TalkCloseReq>(&request.payload_json).map(|value| {
                TalkManager::close(&value.talk_id);
            }),
        )))
    }

    async fn talk_online(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "stream_control.talk_online, req: payload_bytes={}",
            request.payload_json.len()
        );
        Ok(tonic::Response::new(
            match decode_payload::<TalkCloseReq>(&request.payload_json) {
                Ok(value) => StreamBoolResponse {
                    value: TalkManager::is_online(&value.talk_id),
                    error: None,
                },
                Err(error) => StreamBoolResponse {
                    value: false,
                    error: Some(error),
                },
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub struct StreamControlAdapter {
    identity: NodeIdentity,
    receive_endpoint: Endpoint,
    streams: HashMap<String, StreamRuntime>,
    outputs: HashMap<String, OutputRuntime>,
    terminal_subscriptions: HashMap<(String, String), i64>,
    media_tx: Option<mpsc::Sender<u32>>,
}

#[derive(Debug, Clone)]
struct StreamRuntime {
    lease_id: String,
    route_id: String,
    endpoints: Vec<Endpoint>,
    state: StreamState,
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
    fn info(&self) -> OutputInfo {
        OutputInfo {
            output_id: self.output_id.clone(),
            stream_id: self.stream_id.clone(),
            output_type: self.output_type.clone(),
            endpoint: self.endpoint.clone(),
            state: self.state as i32,
            subscription_id: self.subscription_id.clone(),
        }
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
            media_tx: None,
        }
    }

    pub fn with_media_tx(mut self, media_tx: mpsc::Sender<u32>) -> Self {
        self.media_tx = Some(media_tx);
        self
    }

    fn should_attempt_output_creation(&self, stream_id: &str) -> bool {
        self.media_tx.is_some() || self.streams.contains_key(stream_id)
    }

    pub fn start_receive(&mut self, request: StartReceiveRequest) -> StartReceiveResponse {
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
        if let Some(existing) = self.streams.get(&request.stream_id) {
            if existing.lease_id == request.lease_id {
                return start_response(
                    &request.stream_id,
                    existing.state,
                    existing.endpoints.clone(),
                    None,
                );
            }
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
        let endpoints = if request.preferred_endpoints.is_empty() {
            vec![self.receive_endpoint.clone()]
        } else {
            request.preferred_endpoints
        };
        self.streams.insert(
            request.stream_id.clone(),
            StreamRuntime {
                lease_id: request.lease_id,
                route_id: request.route_id,
                endpoints: endpoints.clone(),
                state: StreamState::Receiving,
            },
        );
        start_response(&request.stream_id, StreamState::Receiving, endpoints, None)
    }

    pub fn stop_receive(&mut self, request: StopReceiveRequest) -> StopReceiveResponse {
        match self.streams.get_mut(&request.stream_id) {
            Some(stream) => {
                stream.state = StreamState::Stopped;
                self.outputs
                    .retain(|_, output| output.stream_id != request.stream_id);
                StopReceiveResponse {
                    state: StreamState::Stopped as i32,
                    error: None,
                }
            }
            None => StopReceiveResponse {
                state: StreamState::Stopped as i32,
                error: None,
            },
        }
    }

    pub fn query_stream(&self, request: QueryStreamRequest) -> QueryStreamResponse {
        let state = self
            .streams
            .get(&request.stream_id)
            .map(|stream| stream.state)
            .unwrap_or(StreamState::Stopped);
        let (viewer_count, viewer_formats) = self.viewer_stats(&request.stream_id);
        QueryStreamResponse {
            stream_id: request.stream_id,
            state: state as i32,
            outputs: self.playback_endpoints(),
            playback_id: String::new(),
            playback_generation: 0,
            source_position_ms: 0,
            media_ready: state == StreamState::Receiving,
            terminal_reason: String::new(),
            viewer_count,
            viewer_formats,
        }
    }

    fn viewer_stats(&self, stream_id: &str) -> (u32, Vec<ViewerFormatCount>) {
        let mut viewers = HashSet::new();
        let mut formats = BTreeMap::<String, HashSet<String>>::new();
        for output in self.outputs.values().filter(|output| {
            output.stream_id == stream_id
                && matches!(output.state, OutputState::Preparing | OutputState::Ready)
                && !output.subscription_id.is_empty()
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

    pub fn create_output(&mut self, request: CreateOutputRequest) -> CreateOutputResponse {
        self.prune_terminal_subscriptions(now_ms());
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
            let output = existing.info();
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
            state: OutputState::Ready,
            subscription_id: request.subscription_id,
        };
        let output = runtime.info();
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
                output: Some(runtime.info()),
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
                output: Some(runtime.info()),
            };
        }
        let mut output = runtime.info();
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
                })
                .map(OutputRuntime::info)
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
            let audio_codec = value
                .transcode
                .as_ref()
                .and_then(|transcode| transcode.audio_codec)
                .map(|_| "aac")
                .unwrap_or("");
            let ssrc = Register::init_media(value).map_err(detail_from_error)?;
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
                        state: OutputState::Ready,
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
                .map(|(stream_id, stream)| ResourceReport {
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
                    labels: HashMap::from([
                        ("route_id".to_string(), stream.route_id.clone()),
                        ("lease_id".to_string(), stream.lease_id.clone()),
                    ]),
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

    #[test]
    fn same_input_supports_four_formats_and_multi_user_independent_close() {
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
        let started = control.start_receive(StartReceiveRequest {
            operation: Some(operation("start-live")),
            stream_id: "stream-a".to_string(),
            route_id: "route-a".to_string(),
            lease_id: "lease-a".to_string(),
            expected_stream: Some(node.identity),
            preferred_endpoints: vec![],
        });
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

    #[test]
    fn subscription_release_is_scoped_and_blocks_late_output_creation() {
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
        control.start_receive(StartReceiveRequest {
            operation: Some(operation("start-live")),
            stream_id: "stream-a".to_string(),
            route_id: "route-a".to_string(),
            lease_id: "lease-a".to_string(),
            expected_stream: Some(node.identity),
            preferred_endpoints: vec![],
        });
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

    #[test]
    fn stream_registers_heartbeats_starts_idempotently_and_snapshots() {
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
        };
        assert_eq!(
            control.start_receive(request.clone()).state,
            StreamState::Receiving as i32
        );
        assert_eq!(
            control.start_receive(request).state,
            StreamState::Receiving as i32
        );
        assert_eq!(control.resource_snapshot().resources.len(), 1);
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

    #[test]
    fn stream_rejects_stale_instance_without_touching_existing_state() {
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
        let response = control.start_receive(StartReceiveRequest {
            operation: Some(operation("op-stale")),
            stream_id: "stream-a".to_string(),
            route_id: "route-a".to_string(),
            lease_id: "lease-a".to_string(),
            expected_stream: Some(stale),
            preferred_endpoints: vec![],
        });
        assert_eq!(response.state, StreamState::Failed as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 0);
    }
}
