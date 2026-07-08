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
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::{AuthState, UserAccount};
use crate::core::{GuardError, LeaseState, NodeIdentity, NodeKind, RouteState};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::RouteService;
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointModeRecord, EndpointRecord, PLAYBACK_TOKEN_TTL_MS, RouteRecord};

#[derive(Debug, Clone)]
pub struct GuardControlRpc {
    store: InMemoryGuardStore,
    auth: AuthState,
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
        Self { store, auth }
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
        let allocation = AllocationService::new(self.store.clone())
            .allocate(AllocationRequest {
                request_id: operation_id.clone(),
                capability: request.stream_type.clone(),
                zone: request.constraints.get("zone").cloned(),
            })
            .map_err(status)?;
        let lease_id = format!("lease-{operation_id}");
        let route_id = format!("route-{operation_id}");
        LeaseService::new(self.store.clone())
            .allocate(LeaseRequest {
                lease_id: lease_id.clone(),
                route_id: route_id.clone(),
                resource_id: request.stream_id.clone(),
                idempotency_key: if operation.idempotency_key.is_empty() {
                    operation_id.clone()
                } else {
                    operation.idempotency_key.clone()
                },
                owner: allocation.owner.clone(),
                now_ms: now_ms(),
                ttl_ms: 30_000,
            })
            .map_err(status)?;
        RouteService::new(self.store.clone())
            .create_allocated(RouteRecord {
                route_id: route_id.clone(),
                resource_id: request.stream_id,
                node_id: allocation.owner.node_id.clone(),
                instance_id: allocation.owner.instance_id.clone(),
                state: RouteState::Allocated,
                desired_generation: 1,
                observed_generation: 0,
                observed_sequence: 0,
            })
            .map_err(status)?;
        let node = self
            .store
            .get_node(&allocation.owner.node_id)
            .ok_or_else(|| Status::not_found("allocated node disappeared"))?;
        Ok(Response::new(AllocateStreamResponse {
            lease_id,
            route_id,
            stream_node: Some(proto_identity(&allocation.owner)),
            endpoints: node.endpoints.into_iter().map(proto_endpoint).collect(),
            ttl_ms: 30_000,
        }))
    }

    async fn confirm_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.confirm_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Confirm)
    }

    async fn fail_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.fail_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Fail)
    }

    async fn release_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        let request = request.into_inner();
        debug!("guard_control.release_lease, req:{request:?}");
        self.transition_lease(request, LeaseTransition::Release)
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
        let lease = if ticket.lease_id.is_empty() {
            self.store
                .leases()
                .into_iter()
                .find(|lease| lease.resource_id == request.stream_id)
        } else {
            self.store.get_lease(&ticket.lease_id)
        };
        if !lease.as_ref().is_some_and(|lease| {
            lease.resource_id == request.stream_id && lease.state == LeaseState::Confirmed
        }) {
            self.store.revoke_playback_token(&request.token);
            return Ok(reject_playback(
                "stream_not_active",
                "stream has no confirmed lease",
            ));
        }
        let route = if ticket.route_id.is_empty() {
            self.store
                .routes()
                .into_iter()
                .find(|route| route.resource_id == request.stream_id)
        } else {
            self.store.get_route(&ticket.route_id)
        };
        if !route.as_ref().is_some_and(|route| {
            route.resource_id == request.stream_id && route.state != RouteState::Closed
        }) {
            self.store.revoke_playback_token(&request.token);
            return Ok(reject_playback(
                "stream_not_active",
                "stream route is closed",
            ));
        }
        ticket.expires_at_ms = now_ms() + PLAYBACK_TOKEN_TTL_MS;
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
    fn transition_lease(
        &self,
        request: ProtoLeaseRequest,
        transition: LeaseTransition,
    ) -> Result<Response<LeaseResponse>, Status> {
        if request.lease_id.is_empty() || request.expected_instance_id.is_empty() {
            return Err(Status::invalid_argument(
                "lease_id and expected_instance_id are required",
            ));
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
