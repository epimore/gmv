use gmv_protocol::common::v1::OperationRef;
use gmv_protocol::guard::v1::guard_control_server::GuardControl;
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, LeaseRequest, LeaseState, QueryNodeRequest, QueryRouteRequest,
    RouteState,
};
use guard::core::{NodeIdentity, NodeKind};
use guard::registry::{RegisterRequest, RegistryService};
use guard::runtime::control_rpc::GuardControlRpc;
use guard::store::InMemoryGuardStore;
use guard::store::model::{EndpointModeRecord, EndpointRecord};
use std::collections::HashMap;

#[test]
fn guard_control_allocates_lease_route_and_exposes_registered_endpoints() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            RegistryService::new(store.clone())
                .register(RegisterRequest {
                    identity: NodeIdentity::new("stream-rpc-1", "inst-1", NodeKind::Stream),
                    capabilities: vec!["live".to_string()],
                    endpoints: vec![EndpointRecord {
                        name: "grpc".to_string(),
                        scheme: "http".to_string(),
                        host: "127.0.0.1".to_string(),
                        port: 19082,
                        mode: EndpointModeRecord::Single,
                        labels: HashMap::new(),
                    }],
                    capacity: 4,
                    host_metrics: Default::default(),
                    zone: Some("z1".to_string()),
                    now_ms: 1_000,
                    takeover: false,
                })
                .unwrap();

            let service = GuardControlRpc::new(store.clone());
            let allocation = service
                .allocate_stream(tonic::Request::new(AllocateStreamRequest {
                    operation: Some(OperationRef {
                        operation_id: "op-rpc-1".to_string(),
                        idempotency_key: "idem-rpc-1".to_string(),
                    }),
                    stream_id: "stream-rpc-001".to_string(),
                    stream_type: "live".to_string(),
                    constraints: HashMap::from([("zone".to_string(), "z1".to_string())]),
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(allocation.lease_id, "lease-op-rpc-1");
            assert_eq!(allocation.route_id, "route-op-rpc-1");
            assert_eq!(allocation.endpoints.len(), 1);
            assert_eq!(allocation.endpoints[0].port, 19082);

            let node = service
                .query_node(tonic::Request::new(QueryNodeRequest {
                    node_id: "stream-rpc-1".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(node.endpoints.len(), 1);

            let route = service
                .query_route(tonic::Request::new(QueryRouteRequest {
                    route_id: allocation.route_id.clone(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(route.state, RouteState::Allocated as i32);

            let confirmed = service
                .confirm_lease(tonic::Request::new(LeaseRequest {
                    lease_id: allocation.lease_id,
                    route_id: allocation.route_id,
                    expected_instance_id: "inst-1".to_string(),
                    error: None,
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(confirmed.state, LeaseState::Confirmed as i32);
        });
}
