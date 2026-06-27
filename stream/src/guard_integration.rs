use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
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
use gmv_domain::info::output::OutputEnum;
use gmv_nodec::NodeEventSender;
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity, NodeKind, OperationRef, ResourceRef,
};
use gmv_protocol::guard::v1::{
    EventPriority, NodeEvent, NodeHealth, NodeHeartbeat, NodeResourceSnapshot, NodeToGuardMessage,
    RegisterNodeRequest, ResourceReport, ResourceState, node_to_guard_message,
};
use gmv_protocol::stream::v1::{
    CloseOutputRequest, CloseOutputResponse, CreateOutputRequest, CreateOutputResponse,
    GetPlaybackEndpointsRequest, GetPlaybackEndpointsResponse, QueryStreamRequest,
    QueryStreamResponse, StartReceiveRequest, StartReceiveResponse, StopReceiveRequest,
    StopReceiveResponse, StreamBoolResponse, StreamJsonRequest, StreamJsonResponse, StreamState,
    StreamUnitResponse, stream_control_server::StreamControl,
};

use crate::io::local::mp4::Mp4OutputInnerEvent;
use crate::io::talk::TalkManager;
use crate::state::register::Register;

static GUARD_EVENT_SENDER: OnceLock<NodeEventSender> = OnceLock::new();
static GUARD_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn init_guard_event_sender(sender: NodeEventSender) {
    let _ = GUARD_EVENT_SENDER.set(sender);
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
    let event_id = format!("stream-event-{sequence}");
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
        base::log::warn!("drop guard stream event {topic}: {error}");
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
}

impl StreamGuardNode {
    pub fn new(
        node_id: impl Into<String>,
        instance_id: impl Into<String>,
        host: impl Into<String>,
        guard_endpoint: impl Into<String>,
        http_port: u32,
        rtp_port: u32,
    ) -> Self {
        let host = host.into();
        Self {
            guard_channel: RpcChannelConfig::new(guard_endpoint.into()),
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Stream as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            endpoints: vec![
                endpoint("http", "http", &host, http_port),
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
            capacity: 100,
            zone: String::new(),
            takeover: false,
            config: self.config_summary(),
        }
    }

    fn config_summary(&self) -> HashMap<String, String> {
        HashMap::from([
            ("node_id".to_string(), self.identity.node_id.clone()),
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
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.start_receive(request.into_inner()),
        ))
    }

    async fn stop_receive(
        &self,
        request: tonic::Request<StopReceiveRequest>,
    ) -> Result<tonic::Response<StopReceiveResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.stop_receive(request.into_inner()),
        ))
    }

    async fn query_stream(
        &self,
        request: tonic::Request<QueryStreamRequest>,
    ) -> Result<tonic::Response<QueryStreamResponse>, tonic::Status> {
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.query_stream(request.into_inner()),
        ))
    }

    async fn create_output(
        &self,
        request: tonic::Request<CreateOutputRequest>,
    ) -> Result<tonic::Response<CreateOutputResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.create_output(request.into_inner()),
        ))
    }

    async fn close_output(
        &self,
        request: tonic::Request<CloseOutputRequest>,
    ) -> Result<tonic::Response<CloseOutputResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.close_output(request.into_inner()),
        ))
    }

    async fn get_playback_endpoints(
        &self,
        request: tonic::Request<GetPlaybackEndpointsRequest>,
    ) -> Result<tonic::Response<GetPlaybackEndpointsResponse>, tonic::Status> {
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.get_playback_endpoints(request.into_inner()),
        ))
    }

    async fn init_media(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("stream control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.init_media(request.into_inner()),
        ))
    }

    async fn init_media_ext(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<MediaMap>(&request.into_inner().payload_json).and_then(|value| {
                Register::init_media_ext(value.ssrc, value.ext).map_err(detail_from_error)
            }),
        )))
    }

    async fn stream_online(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            match decode_payload::<StreamKey>(&request.into_inner().payload_json) {
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
        Ok(tonic::Response::new(
            record_info_response(request.into_inner()).await,
        ))
    }

    async fn close_output_by_ssrc(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<StreamInfoQo>(&request.into_inner().payload_json)
                .and_then(close_output_by_ssrc),
        )))
    }

    async fn talk_open(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            match decode_payload::<TalkOpenReq>(&request.into_inner().payload_json) {
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
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<TalkAnswerReq>(&request.into_inner().payload_json)
                .and_then(|value| TalkManager::answer(value).map_err(detail_from_error)),
        )))
    }

    async fn talk_close(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(stream_unit_response(
            decode_payload::<TalkCloseReq>(&request.into_inner().payload_json).map(|value| {
                TalkManager::close(&value.talk_id);
            }),
        )))
    }

    async fn talk_online(
        &self,
        request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            match decode_payload::<TalkCloseReq>(&request.into_inner().payload_json) {
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
    outputs: HashMap<String, String>,
    media_tx: Option<mpsc::Sender<u32>>,
}

#[derive(Debug, Clone)]
struct StreamRuntime {
    lease_id: String,
    route_id: String,
    endpoints: Vec<Endpoint>,
    state: StreamState,
}

impl StreamControlAdapter {
    pub fn new(identity: NodeIdentity, receive_endpoint: Endpoint) -> Self {
        Self {
            identity,
            receive_endpoint,
            streams: HashMap::new(),
            outputs: HashMap::new(),
            media_tx: None,
        }
    }

    pub fn with_media_tx(mut self, media_tx: mpsc::Sender<u32>) -> Self {
        self.media_tx = Some(media_tx);
        self
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
        QueryStreamResponse {
            stream_id: request.stream_id,
            state: state as i32,
            outputs: self.playback_endpoints(),
        }
    }

    pub fn create_output(&mut self, request: CreateOutputRequest) -> CreateOutputResponse {
        if request.endpoint_mode == EndpointMode::Multi as i32 {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error(
                    "multi_endpoint_disabled",
                    "multi RTP endpoint pool is reserved but not enabled",
                )),
            };
        }
        if !self.streams.contains_key(&request.stream_id) {
            return CreateOutputResponse {
                output_id: String::new(),
                endpoints: vec![],
                error: Some(error("stream_not_found", "stream is not receiving")),
            };
        }
        let output_id = format!("out-{}-{}", request.output_type, request.stream_id);
        self.outputs.insert(output_id.clone(), request.stream_id);
        CreateOutputResponse {
            output_id,
            endpoints: self.playback_endpoints(),
            error: None,
        }
    }

    pub fn close_output(&mut self, request: CloseOutputRequest) -> CloseOutputResponse {
        CloseOutputResponse {
            closed: self.outputs.remove(&request.output_id).is_some(),
            error: None,
        }
    }

    pub fn get_playback_endpoints(
        &self,
        _request: GetPlaybackEndpointsRequest,
    ) -> GetPlaybackEndpointsResponse {
        GetPlaybackEndpointsResponse {
            endpoints: self.playback_endpoints(),
        }
    }

    pub fn init_media(&mut self, request: StreamJsonRequest) -> StreamUnitResponse {
        let media_tx = match self.media_tx.as_ref() {
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
        let result = decode_payload::<MediaConfig>(&request.payload_json)
            .and_then(|value| Register::init_media(value).map_err(detail_from_error))
            .and_then(|ssrc| {
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
    error("stream_control_failed", &error_value.to_string())
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

    #[test]
    fn stream_registers_heartbeats_starts_idempotently_and_snapshots() {
        let node = StreamGuardNode::new(
            "stream-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            18080,
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
        assert!(
            control
                .create_output(CreateOutputRequest {
                    operation: Some(operation("out-1")),
                    stream_id: "stream-a".to_string(),
                    output_type: "flv".to_string(),
                    endpoint_mode: EndpointMode::Single as i32
                })
                .error
                .is_none()
        );
        assert!(
            control
                .create_output(CreateOutputRequest {
                    operation: Some(operation("out-2")),
                    stream_id: "stream-a".to_string(),
                    output_type: "rtp".to_string(),
                    endpoint_mode: EndpointMode::Multi as i32
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
