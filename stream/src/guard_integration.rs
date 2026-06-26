use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base_rpc::RpcChannelConfig;
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
    StopReceiveResponse, StreamState, stream_control_server::StreamControl,
};

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
        http_port: u32,
        rtp_port: u32,
    ) -> Self {
        Self {
            guard_channel: RpcChannelConfig::new("http://127.0.0.1:18080"),
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Stream as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            endpoints: vec![
                endpoint("http", "http", http_port),
                endpoint("rtp", "rtp", rtp_port),
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
        }
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
}

#[derive(Debug, Clone)]
pub struct StreamControlAdapter {
    identity: NodeIdentity,
    receive_endpoint: Endpoint,
    streams: HashMap<String, StreamRuntime>,
    outputs: HashMap<String, String>,
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
        }
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

fn endpoint(name: &str, scheme: &str, port: u32) -> Endpoint {
    Endpoint {
        name: name.to_string(),
        scheme: scheme.to_string(),
        host: "127.0.0.1".to_string(),
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
        let node = StreamGuardNode::new("stream-1", "inst-1", 18080, 30000);
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

        let mut control =
            StreamControlAdapter::new(node.identity.clone(), endpoint("rtp", "rtp", 30000));
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
        let node = StreamGuardNode::new("stream-1", "inst-1", 18080, 30000);
        let mut control =
            StreamControlAdapter::new(node.identity.clone(), endpoint("rtp", "rtp", 30000));
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
