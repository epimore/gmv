use gmv_guard_server::auth::{AuthState, Role, SessionPolicy, UserAccount, hash_password};
use gmv_guard_server::core::{
    LeaseState as CoreLeaseState, NodeIdentity, NodeKind, RouteState as CoreRouteState,
};
use gmv_guard_server::registry::{RegisterRequest, RegistryService};
use gmv_guard_server::runtime::control_rpc::GuardControlRpc;
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::model::{
    EndpointModeRecord, EndpointRecord, LeaseRecord, PlaybackTicketRecord, RouteRecord,
};
use gmv_protocol::common::v1::OperationRef;
use gmv_protocol::guard::v1::guard_control_server::GuardControl;
use gmv_protocol::guard::v1::{
    AllocateStreamRequest, CheckPlaybackRequest, LeaseRequest, LeaseState, QueryNodeRequest,
    QueryRouteRequest, RouteState,
};
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
                    host_metrics: Default::default(),
                    zone: Some("z1".to_string()),
                    now_ms: 1_000,
                    takeover: false,
                    config: Default::default(),
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
                    lease_id: allocation.lease_id.clone(),
                    route_id: allocation.route_id.clone(),
                    expected_instance_id: "inst-1".to_string(),
                    error: None,
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(confirmed.state, LeaseState::Confirmed as i32);

            let released = service
                .release_lease(tonic::Request::new(LeaseRequest {
                    lease_id: allocation.lease_id,
                    route_id: allocation.route_id.clone(),
                    expected_instance_id: "inst-1".to_string(),
                    error: None,
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(released.state, LeaseState::Released as i32);
            let route = service
                .query_route(tonic::Request::new(QueryRouteRequest {
                    route_id: allocation.route_id,
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(route.state, RouteState::Closed as i32);
        });
}

#[test]
fn guard_control_checks_playback_ticket_stream_session_and_revocation() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            store
                .insert_lease(LeaseRecord {
                    lease_id: "lease-play-1".to_string(),
                    route_id: "route-play-1".to_string(),
                    resource_id: "stream-play-1".to_string(),
                    stream_type: "live".to_string(),
                    node_id: "stream-rpc-1".to_string(),
                    instance_id: "inst-1".to_string(),
                    idempotency_key: String::new(),
                    constraints: HashMap::new(),
                    state: CoreLeaseState::Confirmed,
                    expires_at_ms: i64::MAX,
                })
                .unwrap();
            store.upsert_route(RouteRecord {
                route_id: "route-play-1".to_string(),
                resource_id: "stream-play-1".to_string(),
                node_id: "stream-rpc-1".to_string(),
                instance_id: "inst-1".to_string(),
                state: CoreRouteState::Running,
                desired_generation: 1,
                observed_generation: 1,
                observed_sequence: 1,
            });
            let auth = AuthState::new(
                [UserAccount::new(
                    "operator",
                    Role::Operator,
                    hash_password("secret").unwrap(),
                )],
                SessionPolicy::default(),
            );
            let (ui_session_token, _) = auth.authenticate("operator", "secret").unwrap();
            store.upsert_playback_ticket(PlaybackTicketRecord {
                token: "play-token-1".to_string(),
                stream_id: "stream-play-1".to_string(),
                playback_id: String::new(),
                playback_start_time_sec: 0,
                playback_end_time_sec: 0,
                output_id: String::new(),
                subscription_id: "viewer-1".to_string(),
                lease_id: "lease-play-1".to_string(),
                route_id: "route-play-1".to_string(),
                username: "operator".to_string(),
                ui_session_token: ui_session_token.clone(),
                required_role: Role::Viewer,
                expires_at_ms: i64::MAX,
            });
            let service = GuardControlRpc::with_auth(store.clone(), auth.clone());

            let accepted = service
                .check_playback(tonic::Request::new(CheckPlaybackRequest {
                    stream_id: "stream-play-1".to_string(),
                    token: "play-token-1".to_string(),
                    remote_addr: "127.0.0.1:30000".to_string(),
                    output_type: "HttpFlv".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert!(accepted.accepted);
            assert!(
                store
                    .get_playback_ticket("play-token-1")
                    .unwrap()
                    .expires_at_ms
                    > 0
            );

            let mismatch = service
                .check_playback(tonic::Request::new(CheckPlaybackRequest {
                    stream_id: "stream-other".to_string(),
                    token: "play-token-1".to_string(),
                    remote_addr: String::new(),
                    output_type: "HttpFlv".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert!(!mismatch.accepted);
            assert_eq!(mismatch.error.unwrap().code, "playback_stream_mismatch");

            store.upsert_playback_ticket(PlaybackTicketRecord {
                token: "play-token-expired".to_string(),
                stream_id: "stream-play-1".to_string(),
                playback_id: String::new(),
                playback_start_time_sec: 0,
                playback_end_time_sec: 0,
                output_id: "output-expired".to_string(),
                subscription_id: "viewer-expired".to_string(),
                lease_id: "lease-play-1".to_string(),
                route_id: "route-play-1".to_string(),
                username: "operator".to_string(),
                ui_session_token: ui_session_token.clone(),
                required_role: Role::Viewer,
                expires_at_ms: 0,
            });
            let expired = service
                .check_playback(tonic::Request::new(CheckPlaybackRequest {
                    stream_id: "stream-play-1".to_string(),
                    token: "play-token-expired".to_string(),
                    remote_addr: String::new(),
                    output_type: "HlsMp4".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert!(!expired.accepted);
            assert_eq!(expired.error.unwrap().code, "playback_token_expired");
            assert!(store.get_playback_ticket("play-token-expired").is_none());

            store.revoke_playback_tickets_for_stream("stream-play-1");
            let revoked = service
                .check_playback(tonic::Request::new(CheckPlaybackRequest {
                    stream_id: "stream-play-1".to_string(),
                    token: "play-token-1".to_string(),
                    remote_addr: String::new(),
                    output_type: "HttpFlv".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert!(!revoked.accepted);
            assert_eq!(revoked.error.unwrap().code, "invalid_playback_token");

            store.upsert_playback_ticket(PlaybackTicketRecord {
                token: "play-token-2".to_string(),
                stream_id: "stream-play-1".to_string(),
                playback_id: String::new(),
                playback_start_time_sec: 0,
                playback_end_time_sec: 0,
                output_id: String::new(),
                subscription_id: "viewer-2".to_string(),
                lease_id: "lease-play-1".to_string(),
                route_id: "route-play-1".to_string(),
                username: "operator".to_string(),
                ui_session_token,
                required_role: Role::Viewer,
                expires_at_ms: i64::MAX,
            });
            auth.revoke_user_sessions("operator");
            let inactive_session = service
                .check_playback(tonic::Request::new(CheckPlaybackRequest {
                    stream_id: "stream-play-1".to_string(),
                    token: "play-token-2".to_string(),
                    remote_addr: String::new(),
                    output_type: "HttpFlv".to_string(),
                }))
                .await
                .unwrap()
                .into_inner();
            assert!(!inactive_session.accepted);
            assert_eq!(inactive_session.error.unwrap().code, "ui_session_inactive");
            assert!(store.get_playback_ticket("play-token-2").is_none());
        });
}
