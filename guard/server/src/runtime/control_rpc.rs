use base::log::debug;
use gmv_protocol::common::v1::{
    Endpoint as ProtoEndpoint, EndpointMode as ProtoEndpointMode, NodeIdentity as ProtoIdentity,
    NodeKind as ProtoNodeKind, ResourceRef,
};
use gmv_protocol::guard::v1::guard_control_server::GuardControl;
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, AllocateStreamResponse, CheckPlaybackRequest, CheckPlaybackResponse,
    LeaseRequest as ProtoLeaseRequest, LeaseResponse, LeaseState as ProtoLeaseState,
    QueryNodeRequest, QueryNodeResponse, QueryRouteRequest, QueryRouteResponse,
    RouteState as ProtoRouteState,
};
use gmv_protocol::stream::v1::{
    MediaTransport, StartReceiveRequest, StartReceiveResponse, StopReceivePhase,
    StopReceiveRequest, StopReceiveResponse, StreamState,
    stream_control_client::StreamControlClient,
};
use std::sync::Arc;
use std::time::Duration;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::{AuthState, UserAccount};
use crate::core::{GuardError, LeaseState, NodeIdentity, NodeKind, RouteState};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::RouteService;
use crate::store::InMemoryGuardStore;
use crate::store::model::{
    EndpointModeRecord, EndpointRecord, LeaseRecord, NodeRecord, PLAYBACK_TOKEN_TTL_MS, RouteRecord,
};

#[tonic::async_trait]
pub trait StreamReceiveControl: std::fmt::Debug + Send + Sync {
    async fn start_receive(
        &self,
        node: &NodeRecord,
        request: StartReceiveRequest,
    ) -> Result<StartReceiveResponse, Status>;

    async fn stop_receive(
        &self,
        node: &NodeRecord,
        request: StopReceiveRequest,
    ) -> Result<StopReceiveResponse, Status>;
}

#[derive(Debug, Default)]
struct RpcStreamReceiveControl;

#[derive(Debug, Clone)]
pub struct GuardControlRpc {
    store: InMemoryGuardStore,
    auth: AuthState,
    stream_control: Arc<dyn StreamReceiveControl>,
}

impl GuardControlRpc {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self::with_auth(
            store,
            AuthState::new(
                std::iter::empty::<UserAccount>(),
                crate::auth::SessionPolicy::default(),
            ),
        )
    }

    pub fn with_auth(store: InMemoryGuardStore, auth: AuthState) -> Self {
        Self {
            store,
            auth,
            stream_control: Arc::new(RpcStreamReceiveControl),
        }
    }

    pub fn with_stream_control(
        store: InMemoryGuardStore,
        auth: AuthState,
        stream_control: Arc<dyn StreamReceiveControl>,
    ) -> Self {
        Self {
            store,
            auth,
            stream_control,
        }
    }

    pub fn new_with_stream_control(
        store: InMemoryGuardStore,
        stream_control: Arc<dyn StreamReceiveControl>,
    ) -> Self {
        Self::with_stream_control(
            store,
            AuthState::new(
                std::iter::empty::<UserAccount>(),
                crate::auth::SessionPolicy::default(),
            ),
            stream_control,
        )
    }
}

#[tonic::async_trait]
impl GuardControl for GuardControlRpc {
    async fn allocate_stream(
        &self,
        request: Request<AllocateStreamRequest>,
    ) -> Result<Response<AllocateStreamResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.allocate_stream, req:{request:?}");
        let operation = request
            .operation
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("operation is required"))?;
        if request.stream_id.is_empty() || request.stream_type.is_empty() {
            return Err(Status::invalid_argument(
                "stream_id and stream_type are required",
            ));
        }
        let operation_id = if operation.operation_id.is_empty() {
            Uuid::now_v7().to_string()
        } else {
            operation.operation_id.clone()
        };
        let idempotency_key = if operation.idempotency_key.is_empty() {
            operation_id.clone()
        } else {
            operation.idempotency_key.clone()
        };
        let lease_id = format!("lease-{operation_id}");
        let route_id = format!("route-{operation_id}");
        if let Some(existing) = self.store.get_lease(&lease_id) {
            if existing.resource_id != request.stream_id
                || existing.stream_type != request.stream_type
                || existing.idempotency_key != idempotency_key
                || existing.route_id != route_id
            {
                return Err(Status::already_exists(format!(
                    "operation {operation_id} conflicts with an existing allocation"
                )));
            }
            if matches!(
                existing.state,
                LeaseState::Failed | LeaseState::Released | LeaseState::Expired
            ) {
                return Err(Status::failed_precondition(format!(
                    "lease {lease_id} is terminal: {:?}",
                    existing.state
                )));
            }
            if !existing.endpoints.is_empty() {
                return Ok(Response::new(allocation_response(existing, 30_000)));
            }
        }
        let constraints = request.constraints.clone();
        let owner = if let Some(existing) = self.store.get_lease(&lease_id) {
            NodeIdentity::new(existing.node_id, existing.instance_id, NodeKind::Stream)
        } else {
            let allocation = AllocationService::new(self.store.clone())
                .allocate(AllocationRequest {
                    request_id: operation_id.clone(),
                    resource_id: request.stream_id.clone(),
                    capability: request.stream_type.clone(),
                    zone: constraints.get("zone").cloned(),
                    constraints: constraints.clone(),
                })
                .map_err(status)?;
            LeaseService::new(self.store.clone())
                .allocate(LeaseRequest {
                    lease_id: lease_id.clone(),
                    route_id: route_id.clone(),
                    resource_id: request.stream_id.clone(),
                    stream_type: request.stream_type.clone(),
                    idempotency_key,
                    owner: allocation.owner.clone(),
                    constraints: constraints.clone(),
                    now_ms: now_ms(),
                    ttl_ms: 30_000,
                })
                .map_err(status)?;
            RouteService::new(self.store.clone())
                .create_allocated(RouteRecord {
                    route_id: route_id.clone(),
                    resource_id: request.stream_id.clone(),
                    node_id: allocation.owner.node_id.clone(),
                    instance_id: allocation.owner.instance_id.clone(),
                    state: RouteState::Allocated,
                    desired_generation: 1,
                    observed_generation: 0,
                    observed_sequence: 0,
                })
                .map_err(status)?;
            allocation.owner
        };
        let node = self
            .store
            .get_node(&owner.node_id)
            .ok_or_else(|| Status::not_found("allocated node disappeared"))?;
        let start_result = start_receive(
            self.stream_control.as_ref(),
            &node,
            &operation_id,
            &request.stream_id,
            &route_id,
            &lease_id,
            constraints,
        )
        .await;
        let receive_endpoints = match start_result {
            Ok(endpoints) => endpoints,
            Err(error) => {
                if let Err(stop_error) = stop_receive(
                    self.stream_control.as_ref(),
                    &node,
                    &operation_id,
                    &request.stream_id,
                    &lease_id,
                    &route_id,
                    "guard_allocation_failed",
                )
                .await
                {
                    base::log::error!(
                        "guard allocation compensation failed: stream_id={}, lease_id={}, reason={stop_error}",
                        request.stream_id,
                        lease_id
                    );
                }
                self.fail_allocation(&lease_id, &route_id, &owner.instance_id);
                return Err(error);
            }
        };
        let mut endpoints = node
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.name != "rtp")
            .cloned()
            .collect::<Vec<_>>();
        endpoints.extend(receive_endpoints);
        let mut lease = self
            .store
            .get_lease(&lease_id)
            .ok_or_else(|| Status::not_found(format!("lease {lease_id}")))?;
        lease.endpoints = endpoints;
        if let Err(error) = self.store.update_lease(lease.clone()) {
            if let Err(stop_error) = stop_receive(
                self.stream_control.as_ref(),
                &node,
                &operation_id,
                &request.stream_id,
                &lease_id,
                &route_id,
                "guard_store_failed",
            )
            .await
            {
                base::log::error!(
                    "guard allocation store compensation failed: stream_id={}, lease_id={}, reason={stop_error}",
                    request.stream_id,
                    lease_id
                );
            }
            self.fail_allocation(&lease_id, &route_id, &owner.instance_id);
            return Err(status(error));
        }
        Ok(Response::new(allocation_response(lease, 30_000)))
    }

    async fn confirm_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.confirm_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Confirm)
            .await
    }

    async fn fail_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.fail_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Fail).await
    }

    async fn release_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.release_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Release)
            .await
    }

    async fn query_node(
        &self,
        request: Request<QueryNodeRequest>,
    ) -> Result<Response<QueryNodeResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.query_node, req:{request:?}");
        let node = self
            .store
            .get_node(&request.node_id)
            .ok_or_else(|| Status::not_found(format!("node {}", request.node_id)))?;
        Ok(Response::new(QueryNodeResponse {
            current: Some(proto_identity(&node.identity)),
            endpoints: node.endpoints.into_iter().map(proto_endpoint).collect(),
        }))
    }

    async fn check_playback(
        &self,
        request: Request<CheckPlaybackRequest>,
    ) -> Result<Response<CheckPlaybackResponse>, Status> {
        let request = request.into_inner();
        debug!(
            "guard_control.check_playback, req: stream_id={}, token={}, remote_addr={}, output_type={}",
            request.stream_id,
            if request.token.is_empty() {
                "<empty>"
            } else {
                "<redacted>"
            },
            request.remote_addr,
            request.output_type
        );
        if request.stream_id.is_empty() || request.token.is_empty() {
            return Ok(Response::new(CheckPlaybackResponse {
                accepted: false,
                error: Some(error_detail(
                    "invalid_playback",
                    "stream_id and token are required",
                )),
            }));
        }
        let Some(mut ticket) = self.store.get_playback_ticket(&request.token) else {
            return Ok(reject_playback(
                "invalid_playback_token",
                "playback token is not valid",
            ));
        };
        let now = now_ms();
        if ticket.expires_at_ms <= now {
            self.store.revoke_playback_token(&request.token);
            return Ok(reject_playback(
                "playback_token_expired",
                "playback token has expired",
            ));
        }
        if ticket.stream_id != request.stream_id {
            return Ok(reject_playback(
                "playback_stream_mismatch",
                "playback token does not match stream",
            ));
        }
        if self
            .auth
            .require_session_token_role(&ticket.ui_session_token, ticket.required_role)
            .is_err()
        {
            self.store.revoke_playback_token(&request.token);
            return Ok(reject_playback(
                "ui_session_inactive",
                "UI session is not active",
            ));
        }
        let allocation = if ticket.lease_id.is_empty() && ticket.route_id.is_empty() {
            self.store
                .resolve_active_allocation(&request.stream_id)
                .ok()
                .flatten()
        } else if !ticket.lease_id.is_empty() && !ticket.route_id.is_empty() {
            self.store
                .get_lease(&ticket.lease_id)
                .zip(self.store.get_route(&ticket.route_id))
        } else {
            None
        };
        if !allocation.as_ref().is_some_and(|(lease, route)| {
            lease.resource_id == request.stream_id
                && lease.state == LeaseState::Confirmed
                && lease.route_id == route.route_id
                && lease.node_id == route.node_id
                && lease.instance_id == route.instance_id
                && route.resource_id == request.stream_id
                && matches!(route.state, RouteState::Allocated | RouteState::Running)
        }) {
            self.store.revoke_playback_token(&request.token);
            return Ok(reject_playback(
                "stream_not_active",
                "stream has no consistent active allocation",
            ));
        }
        ticket.expires_at_ms = now + PLAYBACK_TOKEN_TTL_MS;
        self.store.upsert_playback_ticket(ticket);
        Ok(Response::new(CheckPlaybackResponse {
            accepted: true,
            error: None,
        }))
    }

    async fn query_route(
        &self,
        request: Request<QueryRouteRequest>,
    ) -> Result<Response<QueryRouteResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.query_route, req:{request:?}");
        let route = self
            .store
            .get_route(&request.route_id)
            .ok_or_else(|| Status::not_found(format!("route {}", request.route_id)))?;
        let owner = NodeIdentity::new(
            route.node_id.clone(),
            route.instance_id.clone(),
            NodeKind::Stream,
        );
        Ok(Response::new(QueryRouteResponse {
            route_id: route.route_id,
            resource: Some(ResourceRef {
                resource_id: route.resource_id,
                resource_type: "stream".to_string(),
            }),
            owner: Some(proto_identity(&owner)),
            state: proto_route_state(route.state) as i32,
        }))
    }
}

impl GuardControlRpc {
    fn fail_allocation(&self, lease_id: &str, route_id: &str, instance_id: &str) {
        if let Err(error) = LeaseService::new(self.store.clone()).fail(lease_id, instance_id) {
            base::log::error!(
                "guard allocation lease compensation failed: lease_id={lease_id}, reason={error}"
            );
        }
        if let Some(mut route) = self.store.get_route(route_id) {
            route.state = RouteState::Closed;
            self.store.upsert_route(route);
        }
    }

    async fn transition_lease(
        &self,
        request: ProtoLeaseRequest,
        transition: LeaseTransition,
    ) -> Result<Response<LeaseResponse>, Status> {
        if request.lease_id.is_empty() || request.expected_instance_id.is_empty() {
            return Err(Status::invalid_argument(
                "lease_id and expected_instance_id are required",
            ));
        }
        let current = self
            .store
            .get_lease(&request.lease_id)
            .ok_or_else(|| Status::not_found(format!("lease {}", request.lease_id)))?;
        if current.instance_id != request.expected_instance_id {
            return Err(Status::failed_precondition(format!(
                "lease {} belongs to {} not {}",
                request.lease_id, current.instance_id, request.expected_instance_id
            )));
        }
        if matches!(transition, LeaseTransition::Fail | LeaseTransition::Release)
            && !current.endpoints.is_empty()
            && !matches!(
                current.state,
                LeaseState::Failed | LeaseState::Released | LeaseState::Expired
            )
        {
            let node = self
                .store
                .get_node(&current.node_id)
                .ok_or_else(|| Status::not_found(format!("node {}", current.node_id)))?;
            stop_receive(
                self.stream_control.as_ref(),
                &node,
                &format!("lease-{}", request.lease_id),
                &current.resource_id,
                &current.lease_id,
                &current.route_id,
                match transition {
                    LeaseTransition::Fail => "guard_lease_failed",
                    LeaseTransition::Release => "guard_lease_released",
                    LeaseTransition::Confirm => unreachable!(),
                },
            )
            .await?;
        }
        let lease = match transition {
            LeaseTransition::Confirm => LeaseService::new(self.store.clone())
                .confirm(&request.lease_id, &request.expected_instance_id),
            LeaseTransition::Fail => LeaseService::new(self.store.clone())
                .fail(&request.lease_id, &request.expected_instance_id),
            LeaseTransition::Release => LeaseService::new(self.store.clone())
                .release(&request.lease_id, &request.expected_instance_id),
        }
        .map_err(status)?;
        if matches!(transition, LeaseTransition::Release) {
            if let Some(mut route) = self
                .store
                .get_route(&lease.route_id)
                .filter(|route| route.state != RouteState::Closed)
            {
                route.state = RouteState::Closed;
                self.store.upsert_route(route);
            }
            self.store.remove_stream_session_owner(&lease.resource_id);
        }
        Ok(Response::new(LeaseResponse {
            state: proto_lease_state(lease.state) as i32,
        }))
    }
}

#[derive(Debug, Clone, Copy)]
enum LeaseTransition {
    Confirm,
    Fail,
    Release,
}

fn allocation_response(lease: LeaseRecord, ttl_ms: u64) -> AllocateStreamResponse {
    let owner = NodeIdentity::new(
        lease.node_id.clone(),
        lease.instance_id.clone(),
        NodeKind::Stream,
    );
    AllocateStreamResponse {
        lease_id: lease.lease_id,
        route_id: lease.route_id,
        stream_node: Some(proto_identity(&owner)),
        endpoints: lease.endpoints.into_iter().map(proto_endpoint).collect(),
        ttl_ms,
    }
}

async fn start_receive(
    control: &dyn StreamReceiveControl,
    node: &NodeRecord,
    operation_id: &str,
    stream_id: &str,
    route_id: &str,
    lease_id: &str,
    constraints: std::collections::HashMap<String, String>,
) -> Result<Vec<EndpointRecord>, Status> {
    let media_transport = match constraints
        .get("media_transport")
        .or_else(|| constraints.get("transport"))
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("udp") => MediaTransport::Udp,
        Some("tcp_active") => MediaTransport::TcpActive,
        Some("tcp_passive") => MediaTransport::TcpPassive,
        Some(_) => return Err(Status::invalid_argument("invalid_media_transport")),
    };
    let response = control
        .start_receive(
            node,
            StartReceiveRequest {
                operation: Some(gmv_protocol::common::v1::OperationRef {
                    operation_id: operation_id.to_string(),
                    idempotency_key: operation_id.to_string(),
                }),
                stream_id: stream_id.to_string(),
                route_id: route_id.to_string(),
                lease_id: lease_id.to_string(),
                expected_stream: Some(proto_identity(&node.identity)),
                preferred_endpoints: node.endpoints.iter().cloned().map(proto_endpoint).collect(),
                constraints,
                reservation_ttl_ms: 30_000,
                media_transport: media_transport as i32,
            },
        )
        .await
        .map_err(|error| Status::unavailable(format!("stream StartReceive failed: {error}")))?;
    if let Some(error) = response
        .error
        .filter(|error| !error.code.is_empty() || !error.message.is_empty())
    {
        return Err(if error.code == "endpoint_allocation_failed" {
            Status::resource_exhausted(error.message)
        } else {
            Status::failed_precondition(format!("{}: {}", error.code, error.message))
        });
    }
    if response.state != StreamState::Receiving as i32 {
        return Err(Status::failed_precondition(format!(
            "stream StartReceive returned state {}",
            response.state
        )));
    }
    let endpoints = response
        .receive_endpoints
        .into_iter()
        .map(endpoint_record)
        .collect::<Result<Vec<_>, _>>()?;
    if !endpoints.iter().any(|endpoint| {
        endpoint.name == "rtp"
            && endpoint.mode == EndpointModeRecord::Single
            && !endpoint.host.is_empty()
            && endpoint.port > 0
    }) {
        return Err(Status::failed_precondition(
            "stream StartReceive returned no concrete RTP endpoint",
        ));
    }
    Ok(endpoints)
}

async fn stop_receive(
    control: &dyn StreamReceiveControl,
    node: &NodeRecord,
    operation_id: &str,
    stream_id: &str,
    lease_id: &str,
    route_id: &str,
    reason: &str,
) -> Result<(), Status> {
    let response = control
        .stop_receive(
            node,
            StopReceiveRequest {
                operation: Some(gmv_protocol::common::v1::OperationRef {
                    operation_id: format!("{operation_id}-compensate"),
                    idempotency_key: format!("{operation_id}-compensate"),
                }),
                stream_id: stream_id.to_string(),
                reason: reason.to_string(),
                phase: StopReceivePhase::Unspecified as i32,
                expected_lease_id: lease_id.to_string(),
                expected_route_id: route_id.to_string(),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| Status::unavailable(format!("stream StopReceive failed: {error}")))?;
    if response.state != StreamState::Stopped as i32 {
        return Err(Status::failed_precondition(format!(
            "stream StopReceive returned state {}",
            response.state
        )));
    }
    Ok(())
}

fn grpc_uri(node: &NodeRecord) -> Result<String, Status> {
    let endpoint = node
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| Status::failed_precondition("stream grpc endpoint missing"))?;
    let scheme = if endpoint.scheme == "grpcs" {
        "https"
    } else {
        "http"
    };
    Ok(format!("{scheme}://{}:{}", endpoint.host, endpoint.port))
}

async fn connect_rpc(uri: &str) -> Result<tonic::transport::Channel, Status> {
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
        .map_err(|error| Status::unavailable(format!("connect stream RPC failed: {error}")))
}

#[tonic::async_trait]
impl StreamReceiveControl for RpcStreamReceiveControl {
    async fn start_receive(
        &self,
        node: &NodeRecord,
        request: StartReceiveRequest,
    ) -> Result<StartReceiveResponse, Status> {
        let uri = grpc_uri(node)?;
        StreamControlClient::new(connect_rpc(&uri).await?)
            .start_receive(request)
            .await
            .map(tonic::Response::into_inner)
    }

    async fn stop_receive(
        &self,
        node: &NodeRecord,
        request: StopReceiveRequest,
    ) -> Result<StopReceiveResponse, Status> {
        let uri = grpc_uri(node)?;
        StreamControlClient::new(connect_rpc(&uri).await?)
            .stop_receive(request)
            .await
            .map(tonic::Response::into_inner)
    }
}

fn endpoint_record(endpoint: ProtoEndpoint) -> Result<EndpointRecord, Status> {
    if endpoint.host.is_empty() || endpoint.port == 0 {
        return Err(Status::failed_precondition(
            "stream returned an invalid endpoint",
        ));
    }
    let mode = match ProtoEndpointMode::try_from(endpoint.mode)
        .unwrap_or(ProtoEndpointMode::Unspecified)
    {
        ProtoEndpointMode::Single => EndpointModeRecord::Single,
        ProtoEndpointMode::Multi => EndpointModeRecord::Multi,
        ProtoEndpointMode::Unspecified => {
            return Err(Status::failed_precondition(
                "stream returned endpoint mode unspecified",
            ));
        }
    };
    Ok(EndpointRecord {
        name: endpoint.name,
        scheme: endpoint.scheme,
        host: endpoint.host,
        port: endpoint.port,
        mode,
        labels: endpoint.labels,
    })
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

fn proto_endpoint(endpoint: EndpointRecord) -> ProtoEndpoint {
    ProtoEndpoint {
        name: endpoint.name,
        scheme: endpoint.scheme,
        host: endpoint.host,
        port: endpoint.port,
        mode: match endpoint.mode {
            EndpointModeRecord::Single => ProtoEndpointMode::Single,
            EndpointModeRecord::Multi => ProtoEndpointMode::Multi,
        } as i32,
        labels: endpoint.labels,
    }
}

fn proto_lease_state(state: LeaseState) -> ProtoLeaseState {
    match state {
        LeaseState::Allocated => ProtoLeaseState::Pending,
        LeaseState::Confirmed => ProtoLeaseState::Confirmed,
        LeaseState::Failed => ProtoLeaseState::Failed,
        LeaseState::Released => ProtoLeaseState::Released,
        LeaseState::Expired => ProtoLeaseState::Expired,
    }
}

fn proto_route_state(state: RouteState) -> ProtoRouteState {
    match state {
        RouteState::Allocated => ProtoRouteState::Allocated,
        RouteState::Running => ProtoRouteState::Running,
        RouteState::Reconciling | RouteState::Conflict => ProtoRouteState::Reconciling,
        RouteState::Closed => ProtoRouteState::Closed,
        RouteState::Orphaned => ProtoRouteState::Orphaned,
    }
}

fn status(error: GuardError) -> Status {
    match error {
        GuardError::Conflict(message) => Status::already_exists(message),
        GuardError::StaleInstance(message) => Status::failed_precondition(message),
        GuardError::NotFound(message) => Status::not_found(message),
        GuardError::Capacity(message) => Status::resource_exhausted(message),
        other => Status::invalid_argument(other.to_string()),
    }
}

fn reject_playback(code: &str, message: &str) -> Response<CheckPlaybackResponse> {
    Response::new(CheckPlaybackResponse {
        accepted: false,
        error: Some(error_detail(code, message)),
    })
}

fn error_detail(code: &str, message: &str) -> gmv_protocol::common::v1::ErrorDetail {
    gmv_nodec::error::error_detail(code, message)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
