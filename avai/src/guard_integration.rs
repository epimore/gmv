use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base_rpc::RpcChannelConfig;
use gmv_protocol::avai::v1::{
    AiTaskState, CancelTaskRequest, CancelTaskResponse, CreateTaskRequest, CreateTaskResponse,
    QueryCapabilitiesRequest, QueryCapabilitiesResponse, QueryTaskRequest, QueryTaskResponse,
    avai_control_server::AvaiControl,
};
use gmv_protocol::common::v1::{
    Endpoint, EndpointMode, ErrorDetail, NodeIdentity, NodeKind, OperationRef, PageResponse,
    ResourceRef,
};
use gmv_protocol::guard::v1::{
    EventPriority, NodeEvent, NodeHealth, NodeHeartbeat, NodeResourceSnapshot, NodeToGuardMessage,
    RegisterNodeRequest, ResourceReport, ResourceState, node_to_guard_message,
};

#[derive(Debug, Clone)]
pub struct AvaiGuardNode {
    pub guard_channel: RpcChannelConfig,
    pub identity: NodeIdentity,
    pub software_version: String,
    pub started_at_epoch_ms: i64,
    pub endpoints: Vec<Endpoint>,
    pub capabilities: Vec<String>,
}

impl AvaiGuardNode {
    pub fn new(
        node_id: impl Into<String>,
        instance_id: impl Into<String>,
        host: impl Into<String>,
        guard_endpoint: impl Into<String>,
        grpc_port: u32,
        capabilities: Vec<String>,
    ) -> Self {
        let host = host.into();
        Self {
            guard_channel: RpcChannelConfig::new(guard_endpoint.into()),
            identity: NodeIdentity {
                node_id: node_id.into(),
                instance_id: instance_id.into(),
                kind: NodeKind::Avai as i32,
            },
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_epoch_ms: 0,
            endpoints: vec![Endpoint {
                name: "grpc".to_string(),
                scheme: "grpc".to_string(),
                host,
                port: grpc_port,
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            }],
            capabilities,
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
            takeover: cfg!(debug_assertions),
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
        running_tasks: usize,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence,
            sent_at_epoch_ms,
            payload: Some(node_to_guard_message::Payload::Heartbeat(NodeHeartbeat {
                health: NodeHealth::Ready as i32,
                host_metrics: None,
                metrics: HashMap::from([("running_tasks".to_string(), running_tasks.to_string())]),
            })),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameReference {
    pub frame_ref: String,
    pub stream_id: String,
    pub expires_at_epoch_ms: i64,
}

impl FrameReference {
    pub fn encode(&self) -> Vec<u8> {
        format!(
            "frame_ref={};stream_id={};expires_at_epoch_ms={}",
            self.frame_ref, self.stream_id, self.expires_at_epoch_ms
        )
        .into_bytes()
    }

    pub fn decode(payload: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(payload).ok()?;
        let mut frame_ref = None;
        let mut stream_id = None;
        let mut expires_at_epoch_ms = None;
        for part in text.split(';') {
            let (key, value) = part.split_once('=')?;
            match key {
                "frame_ref" => frame_ref = Some(value.to_string()),
                "stream_id" => stream_id = Some(value.to_string()),
                "expires_at_epoch_ms" => expires_at_epoch_ms = value.parse::<i64>().ok(),
                _ => {}
            }
        }
        Some(Self {
            frame_ref: frame_ref?,
            stream_id: stream_id?,
            expires_at_epoch_ms: expires_at_epoch_ms?,
        })
    }
}

#[derive(Clone)]
pub struct AvaiControlRpc {
    inner: Arc<Mutex<AvaiControlAdapter>>,
}

impl AvaiControlRpc {
    pub fn new(adapter: AvaiControlAdapter) -> Self {
        Self {
            inner: Arc::new(Mutex::new(adapter)),
        }
    }

    pub fn running_task_count(&self) -> usize {
        self.inner
            .lock()
            .map_or(0, |adapter| adapter.running_task_count())
    }
}

#[tonic::async_trait]
impl AvaiControl for AvaiControlRpc {
    async fn create_task(
        &self,
        request: tonic::Request<CreateTaskRequest>,
    ) -> Result<tonic::Response<CreateTaskResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!(
            "avai_control.create_task, req: operation={:?}, task_id={}, task_type={}, route_id={}, expected_avai={:?}, payload_bytes={}",
            request.operation,
            request.task_id,
            request.task_type,
            request.route_id,
            request.expected_avai,
            request.payload.len()
        );
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("avai control lock poisoned"))?;
        Ok(tonic::Response::new(
            control.create_task(request, now_epoch_ms()),
        ))
    }

    async fn cancel_task(
        &self,
        request: tonic::Request<CancelTaskRequest>,
    ) -> Result<tonic::Response<CancelTaskResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("avai_control.cancel_task, req:{request:?}");
        let mut control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("avai control lock poisoned"))?;
        Ok(tonic::Response::new(control.cancel_task(request)))
    }

    async fn query_task(
        &self,
        request: tonic::Request<QueryTaskRequest>,
    ) -> Result<tonic::Response<QueryTaskResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("avai_control.query_task, req:{request:?}");
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("avai control lock poisoned"))?;
        Ok(tonic::Response::new(control.query_task(request)))
    }

    async fn query_capabilities(
        &self,
        request: tonic::Request<QueryCapabilitiesRequest>,
    ) -> Result<tonic::Response<QueryCapabilitiesResponse>, tonic::Status> {
        let request = request.into_inner();
        base::log::debug!("avai_control.query_capabilities, req:{request:?}");
        let control = self
            .inner
            .lock()
            .map_err(|_| tonic::Status::internal("avai control lock poisoned"))?;
        Ok(tonic::Response::new(control.query_capabilities(request)))
    }
}

#[derive(Debug, Clone)]
pub struct AvaiControlAdapter {
    identity: NodeIdentity,
    capabilities: Vec<String>,
    tasks: HashMap<String, AiTask>,
}

#[derive(Debug, Clone)]
struct AiTask {
    task_type: String,
    route_id: String,
    frame: Option<FrameReference>,
    state: AiTaskState,
    result: Vec<u8>,
}

impl AvaiControlAdapter {
    pub fn new(identity: NodeIdentity, capabilities: Vec<String>) -> Self {
        Self {
            identity,
            capabilities,
            tasks: HashMap::new(),
        }
    }

    pub fn create_task(&mut self, request: CreateTaskRequest, now_ms: i64) -> CreateTaskResponse {
        if !self.matches_expected(request.expected_avai.as_ref()) {
            return create_response(
                &request.task_id,
                AiTaskState::Failed,
                Some(error("stale_instance", "avai instance does not match")),
            );
        }
        if !self
            .capabilities
            .iter()
            .any(|capability| capability == &request.task_type)
        {
            return create_response(
                &request.task_id,
                AiTaskState::Failed,
                Some(error(
                    "capability_not_found",
                    "model capability is not available",
                )),
            );
        }
        if let Some(existing) = self.tasks.get(&request.task_id) {
            return create_response(&request.task_id, existing.state, None);
        }
        let frame = if request.payload.is_empty() {
            None
        } else {
            FrameReference::decode(&request.payload)
        };
        if let Some(frame) = &frame
            && frame.expires_at_epoch_ms <= now_ms
        {
            return create_response(
                &request.task_id,
                AiTaskState::Failed,
                Some(error("frame_expired", "frame reference has expired")),
            );
        }
        let _ = (request.task_type, request.route_id, frame);
        create_response(
            &request.task_id,
            AiTaskState::Failed,
            Some(error(
                "executor_unavailable",
                "avai model executor is not configured",
            )),
        )
    }

    pub fn cancel_task(&mut self, request: CancelTaskRequest) -> CancelTaskResponse {
        let state = match self.tasks.get_mut(&request.task_id) {
            Some(task) => {
                task.state = AiTaskState::Cancelled;
                AiTaskState::Cancelled
            }
            None => AiTaskState::Cancelled,
        };
        CancelTaskResponse {
            state: state as i32,
            error: None,
        }
    }

    pub fn query_task(&self, request: QueryTaskRequest) -> QueryTaskResponse {
        match self.tasks.get(&request.task_id) {
            Some(task) => QueryTaskResponse {
                task_id: request.task_id,
                state: task.state as i32,
                result: task.result.clone(),
                error: None,
            },
            None => QueryTaskResponse {
                task_id: request.task_id,
                state: AiTaskState::Failed as i32,
                result: vec![],
                error: Some(error("task_not_found", "task does not exist")),
            },
        }
    }

    pub fn running_task_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| task.state == AiTaskState::Running)
            .count()
    }

    pub fn query_capabilities(
        &self,
        _request: QueryCapabilitiesRequest,
    ) -> QueryCapabilitiesResponse {
        QueryCapabilitiesResponse {
            capabilities: self.capabilities.clone(),
            page: Some(PageResponse {
                next_page_token: String::new(),
            }),
        }
    }

    pub fn complete_task(&mut self, task_id: &str, result: Vec<u8>) -> Option<NodeToGuardMessage> {
        let task = self.tasks.get_mut(task_id)?;
        task.state = AiTaskState::Succeeded;
        task.result = result.clone();
        Some(self.task_event(task_id, EventPriority::P1, result))
    }

    pub fn progress_event(&self, task_id: &str, progress: u8) -> Option<NodeToGuardMessage> {
        self.tasks.get(task_id)?;
        Some(self.task_event(
            task_id,
            EventPriority::P2,
            format!("progress={progress}").into_bytes(),
        ))
    }

    pub fn resource_snapshot(&self) -> NodeResourceSnapshot {
        NodeResourceSnapshot {
            full: true,
            resources: self
                .tasks
                .iter()
                .map(|(task_id, task)| ResourceReport {
                    resource: Some(ResourceRef {
                        resource_id: task_id.clone(),
                        resource_type: "ai_task".to_string(),
                    }),
                    state: match task.state {
                        AiTaskState::Running => ResourceState::Running as i32,
                        AiTaskState::Succeeded | AiTaskState::Cancelled => {
                            ResourceState::Stopped as i32
                        }
                        AiTaskState::Failed => ResourceState::Failed as i32,
                        _ => ResourceState::Starting as i32,
                    },
                    labels: labels_for(task),
                })
                .collect(),
        }
    }

    fn task_event(
        &self,
        task_id: &str,
        priority: EventPriority,
        payload: Vec<u8>,
    ) -> NodeToGuardMessage {
        NodeToGuardMessage {
            identity: Some(self.identity.clone()),
            sequence: 1,
            sent_at_epoch_ms: 0,
            payload: Some(node_to_guard_message::Payload::Event(NodeEvent {
                event_id: format!("ai-{task_id}-{}", priority as i32),
                topic: "avai.task.result".to_string(),
                priority: priority as i32,
                payload,
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

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn labels_for(task: &AiTask) -> HashMap<String, String> {
    let mut labels = HashMap::from([
        ("task_type".to_string(), task.task_type.clone()),
        ("route_id".to_string(), task.route_id.clone()),
    ]);
    if let Some(frame) = &task.frame {
        labels.insert("frame_ref".to_string(), frame.frame_ref.clone());
        labels.insert("stream_id".to_string(), frame.stream_id.clone());
    }
    labels
}

fn create_response(
    task_id: &str,
    state: AiTaskState,
    error: Option<ErrorDetail>,
) -> CreateTaskResponse {
    CreateTaskResponse {
        task_id: task_id.to_string(),
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

    #[test]
    fn avai_registers_filters_capabilities_and_handles_idempotent_tasks() {
        let node = AvaiGuardNode::new(
            "avai-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            19090,
            vec!["ai.vehicle".to_string()],
        );
        let register = node.register_request(NodeResourceSnapshot {
            resources: vec![],
            full: true,
        });
        assert_eq!(register.identity.unwrap().kind, NodeKind::Avai as i32);
        assert_eq!(register.capabilities, vec!["ai.vehicle".to_string()]);
        let heartbeat = node.heartbeat_message(1, 1000, 0);
        assert!(matches!(
            heartbeat.payload,
            Some(node_to_guard_message::Payload::Heartbeat(_))
        ));

        let mut control = AvaiControlAdapter::new(node.identity.clone(), node.capabilities.clone());
        let frame = FrameReference {
            frame_ref: "frame-1".to_string(),
            stream_id: "stream-1".to_string(),
            expires_at_epoch_ms: 2000,
        };
        let request = CreateTaskRequest {
            operation: Some(operation("ai-1")),
            task_id: "task-1".to_string(),
            task_type: "ai.vehicle".to_string(),
            route_id: "route-1".to_string(),
            expected_avai: Some(node.identity.clone()),
            payload: frame.encode(),
        };
        let response = control.create_task(request.clone(), 1000);
        assert_eq!(response.state, AiTaskState::Failed as i32);
        assert_eq!(response.error.unwrap().code, "executor_unavailable");
        let repeated = control.create_task(request, 1000);
        assert_eq!(repeated.state, AiTaskState::Failed as i32);
        assert_eq!(control.resource_snapshot().resources.len(), 0);
        assert!(control.progress_event("task-1", 50).is_none());
        assert!(control.complete_task("task-1", b"ok".to_vec()).is_none());
    }

    #[test]
    fn avai_rejects_expired_frame_unknown_model_and_stale_instance() {
        let node = AvaiGuardNode::new(
            "avai-1",
            "inst-1",
            "127.0.0.1",
            "http://127.0.0.1:18080",
            19090,
            vec!["ai.vehicle".to_string()],
        );
        let mut control = AvaiControlAdapter::new(node.identity.clone(), node.capabilities.clone());
        let expired = FrameReference {
            frame_ref: "frame-old".to_string(),
            stream_id: "stream-1".to_string(),
            expires_at_epoch_ms: 10,
        };
        let response = control.create_task(
            CreateTaskRequest {
                operation: Some(operation("expired")),
                task_id: "task-expired".to_string(),
                task_type: "ai.vehicle".to_string(),
                route_id: "route-1".to_string(),
                expected_avai: Some(node.identity.clone()),
                payload: expired.encode(),
            },
            20,
        );
        assert_eq!(response.state, AiTaskState::Failed as i32);
        let missing = control.create_task(
            CreateTaskRequest {
                operation: Some(operation("missing")),
                task_id: "task-missing".to_string(),
                task_type: "ai.face".to_string(),
                route_id: "route-1".to_string(),
                expected_avai: Some(node.identity.clone()),
                payload: vec![],
            },
            20,
        );
        assert_eq!(missing.state, AiTaskState::Failed as i32);
        let stale = NodeIdentity {
            node_id: "avai-1".to_string(),
            instance_id: "old".to_string(),
            kind: NodeKind::Avai as i32,
        };
        let stale_response = control.create_task(
            CreateTaskRequest {
                operation: Some(operation("stale")),
                task_id: "task-stale".to_string(),
                task_type: "ai.vehicle".to_string(),
                route_id: "route-1".to_string(),
                expected_avai: Some(stale),
                payload: vec![],
            },
            20,
        );
        assert_eq!(stale_response.state, AiTaskState::Failed as i32);
    }
}
