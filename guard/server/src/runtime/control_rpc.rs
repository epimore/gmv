use gmv_protocol::common::v1::{
    Endpoint as ProtoEndpoint, EndpointMode as ProtoEndpointMode, NodeIdentity as ProtoIdentity,
    NodeKind as ProtoNodeKind, ResourceRef,
};
use gmv_protocol::guard::v1::guard_control_server::GuardControl;
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, AllocateStreamResponse, LeaseRequest as ProtoLeaseRequest,
    LeaseResponse, LeaseState as ProtoLeaseState, QueryNodeRequest, QueryNodeResponse,
    QueryRouteRequest, QueryRouteResponse, RouteState as ProtoRouteState,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::core::{GuardError, LeaseState, NodeIdentity, NodeKind, RouteState};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::RouteService;
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointModeRecord, EndpointRecord, RouteRecord};

#[derive(Debug, Clone)]
pub struct GuardControlRpc {
    store: InMemoryGuardStore,
}

impl GuardControlRpc {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }
}

#[tonic::async_trait]
impl GuardControl for GuardControlRpc {
    async fn allocate_stream(
        &self,
        request: Request<AllocateStreamRequest>,
    ) -> Result<Response<AllocateStreamResponse>, Status> {
        let request = request.into_inner();
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
        self.transition_lease(request.into_inner(), LeaseTransition::Confirm)
    }

    async fn fail_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        self.transition_lease(request.into_inner(), LeaseTransition::Fail)
    }

    async fn release_lease(
        &self,
        request: Request<ProtoLeaseRequest>,
    ) -> Result<Response<LeaseResponse>, Status> {
        self.transition_lease(request.into_inner(), LeaseTransition::Release)
    }

    async fn query_node(
        &self,
        request: Request<QueryNodeRequest>,
    ) -> Result<Response<QueryNodeResponse>, Status> {
        let request = request.into_inner();
        let node = self
            .store
            .get_node(&request.node_id)
            .ok_or_else(|| Status::not_found(format!("node {}", request.node_id)))?;
        Ok(Response::new(QueryNodeResponse {
            current: Some(proto_identity(&node.identity)),
            endpoints: node.endpoints.into_iter().map(proto_endpoint).collect(),
        }))
    }

    async fn query_route(
        &self,
        request: Request<QueryRouteRequest>,
    ) -> Result<Response<QueryRouteResponse>, Status> {
        let request = request.into_inner();
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
