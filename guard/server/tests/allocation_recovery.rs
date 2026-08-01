use std::collections::HashMap;

use gmv_guard_server::core::{LeaseState, NodeIdentity, NodeKind, RouteState};
use gmv_guard_server::gateway::{AllocationRequest, AllocationService};
use gmv_guard_server::lease::{LeaseRequest, LeaseService};
use gmv_guard_server::registry::{RegisterRequest, RegistryService};
use gmv_guard_server::route::{RecoveryIssue, ResourceSnapshot, RouteService, SnapshotResource};
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::model::{EndpointModeRecord, EndpointRecord, RouteRecord};

fn stream_identity(node_id: &str, instance_id: &str) -> NodeIdentity {
    NodeIdentity::new(node_id, instance_id, NodeKind::Stream)
}

fn register_stream(store: &InMemoryGuardStore, node_id: &str, instance_id: &str) -> NodeIdentity {
    register_stream_with_config(store, node_id, instance_id, HashMap::new())
}

fn register_stream_with_config(
    store: &InMemoryGuardStore,
    node_id: &str,
    instance_id: &str,
    config: HashMap<String, String>,
) -> NodeIdentity {
    let identity = stream_identity(node_id, instance_id);
    RegistryService::new(store.clone())
        .register(RegisterRequest {
            identity: identity.clone(),
            capabilities: vec!["live".to_string(), "broadcast".to_string()],
            endpoints: vec![EndpointRecord {
                name: "rtp".to_string(),
                scheme: "rtp".to_string(),
                host: "127.0.0.1".to_string(),
                port: 30_000,
                mode: EndpointModeRecord::Multi,
                labels: HashMap::from([(
                    "media_transports".to_string(),
                    "udp,tcp_active,tcp_passive".to_string(),
                )]),
            }],
            host_metrics: Default::default(),
            zone: Some("z1".to_string()),
            now_ms: 1_000,
            takeover: false,
            config,
        })
        .unwrap();
    identity
}

#[test]
fn allocation_filters_scores_and_explains_selection() {
    let store = InMemoryGuardStore::default();
    let left = register_stream(&store, "stream-a", "inst-a");
    let _right = register_stream(&store, "stream-b", "inst-b");
    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-1".to_string(),
            resource_id: "stream-req-1".to_string(),
            capability: "live".to_string(),
            zone: Some("z1".to_string()),
            constraints: HashMap::new(),
        })
        .unwrap();

    assert!(matches!(
        result.owner.node_id.as_str(),
        "stream-a" | "stream-b"
    ));
    assert_eq!(result.explain.selected_node_id, result.owner.node_id);
    assert!(
        result
            .explain
            .scores
            .iter()
            .any(|score| score.node_id == left.node_id)
    );
}

#[test]
fn broadcast_leg_is_pinned_to_expected_stream_owner() {
    let store = InMemoryGuardStore::default();
    register_stream(&store, "stream-a", "inst-a");
    let expected = register_stream(&store, "stream-b", "inst-b");

    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-pinned-broadcast".to_string(),
            resource_id: "broadcast-leg-1".to_string(),
            capability: "broadcast".to_string(),
            zone: Some("z1".to_string()),
            constraints: HashMap::from([(
                "expected_stream_node_id".to_string(),
                expected.node_id.clone(),
            )]),
        })
        .unwrap();

    assert_eq!(result.owner, expected);
}

#[test]
fn allocation_prefers_lower_active_lease_load() {
    let store = InMemoryGuardStore::default();
    let left = register_stream(&store, "stream-a", "inst-a");
    let right = register_stream(&store, "stream-b", "inst-b");
    let leases = LeaseService::new(store.clone());
    leases
        .allocate(LeaseRequest {
            lease_id: "lease-active-a".to_string(),
            route_id: "route-active-a".to_string(),
            resource_id: "stream-active-a".to_string(),
            stream_type: "live".to_string(),
            idempotency_key: "idem-active-a".to_string(),
            owner: left,
            constraints: HashMap::new(),
            now_ms: 1_000,
            ttl_ms: 30_000,
        })
        .unwrap();
    leases.confirm("lease-active-a", "inst-a").unwrap();

    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-low-load".to_string(),
            resource_id: "stream-low-load".to_string(),
            capability: "live".to_string(),
            zone: Some("z1".to_string()),
            constraints: HashMap::new(),
        })
        .unwrap();

    assert_eq!(result.owner, right);
    let left_score = result
        .explain
        .scores
        .iter()
        .find(|score| score.node_id == "stream-a")
        .unwrap();
    assert_eq!(left_score.active_confirmed, 1);
}

#[test]
fn allocation_skips_multi_node_with_exhausted_media_pool() {
    let store = InMemoryGuardStore::default();
    register_stream(&store, "stream-full", "inst-full");
    let available = register_stream(&store, "stream-available", "inst-available");
    for (node_id, free) in [("stream-full", "0"), ("stream-available", "1")] {
        let mut node = store.get_node(node_id).unwrap();
        node.endpoints.push(EndpointRecord {
            name: "rtp".to_string(),
            scheme: "rtp".to_string(),
            host: "127.0.0.1".to_string(),
            port: 28600,
            mode: EndpointModeRecord::Multi,
            labels: HashMap::from([
                ("port_range_start".to_string(), "28600".to_string()),
                ("port_range_end".to_string(), "28601".to_string()),
            ]),
        });
        node.business_metrics
            .insert("media_ports_free".to_string(), free.to_string());
        store.upsert_node(node);
    }

    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-capacity".to_string(),
            resource_id: "stream-capacity".to_string(),
            capability: "live".to_string(),
            zone: Some("z1".to_string()),
            constraints: HashMap::new(),
        })
        .unwrap();

    assert_eq!(result.owner, available);
    let full = result
        .explain
        .scores
        .iter()
        .find(|score| score.node_id == "stream-full")
        .unwrap();
    assert!(!full.eligible);
    assert_eq!(full.reason, "media_port_pool_exhausted");
}

#[test]
fn tcp_passive_broadcast_uses_distinct_stream_node() {
    let store = InMemoryGuardStore::default();
    let left = register_stream(&store, "stream-a", "inst-a");
    let right = register_stream(&store, "stream-b", "inst-b");
    let constraints = HashMap::from([
        ("transport".to_string(), "tcp_passive".to_string()),
        (
            "requires_dedicated_media_endpoint".to_string(),
            "true".to_string(),
        ),
    ]);
    LeaseService::new(store.clone())
        .allocate(LeaseRequest {
            lease_id: "lease-broadcast-a".to_string(),
            route_id: "route-broadcast-a".to_string(),
            resource_id: "broadcast-a".to_string(),
            stream_type: "broadcast".to_string(),
            idempotency_key: "idem-broadcast-a".to_string(),
            owner: left,
            constraints: constraints.clone(),
            now_ms: 1_000,
            ttl_ms: 30_000,
        })
        .unwrap();

    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-broadcast-b".to_string(),
            resource_id: "broadcast-b".to_string(),
            capability: "broadcast".to_string(),
            zone: Some("z1".to_string()),
            constraints,
        })
        .unwrap();

    assert_eq!(result.owner, right);
    let left_score = result
        .explain
        .scores
        .iter()
        .find(|score| score.node_id == "stream-a")
        .unwrap();
    assert!(!left_score.eligible);
    assert_eq!(left_score.reason, "tcp_passive_domain_busy");
}

#[test]
fn drained_stream_node_is_not_allocated() {
    let store = InMemoryGuardStore::default();
    register_stream_with_config(
        &store,
        "stream-a",
        "inst-a",
        HashMap::from([("drain".to_string(), "true".to_string())]),
    );
    let right = register_stream(&store, "stream-b", "inst-b");

    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-drain".to_string(),
            resource_id: "stream-drain".to_string(),
            capability: "live".to_string(),
            zone: Some("z1".to_string()),
            constraints: HashMap::new(),
        })
        .unwrap();

    assert_eq!(result.owner, right);
    assert!(
        result
            .explain
            .scores
            .iter()
            .all(|score| score.node_id != "stream-a")
    );
}

#[test]
fn lease_state_machine_rejects_stale_instance_and_expires() {
    let store = InMemoryGuardStore::default();
    let owner = register_stream(&store, "stream-a", "inst-a");
    let service = LeaseService::new(store.clone());
    service
        .allocate(LeaseRequest {
            lease_id: "lease-1".to_string(),
            route_id: "route-1".to_string(),
            resource_id: "stream-001".to_string(),
            stream_type: "live".to_string(),
            idempotency_key: "idem-1".to_string(),
            owner: owner.clone(),
            constraints: HashMap::new(),
            now_ms: 1_000,
            ttl_ms: 30_000,
        })
        .unwrap();
    assert!(service.confirm("lease-1", "old-inst").is_err());
    assert_eq!(
        service
            .confirm("lease-1", &owner.instance_id)
            .unwrap()
            .state,
        LeaseState::Confirmed
    );

    service
        .allocate(LeaseRequest {
            lease_id: "lease-2".to_string(),
            route_id: "route-2".to_string(),
            resource_id: "stream-002".to_string(),
            stream_type: "live".to_string(),
            idempotency_key: "idem-2".to_string(),
            owner,
            constraints: HashMap::new(),
            now_ms: 1_000,
            ttl_ms: 10,
        })
        .unwrap();
    RouteService::new(store.clone())
        .create_allocated(RouteRecord {
            route_id: "route-2".to_string(),
            resource_id: "stream-002".to_string(),
            node_id: "stream-a".to_string(),
            instance_id: "inst-a".to_string(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })
        .unwrap();
    assert_eq!(service.expire_due(1_011), vec!["lease-2".to_string()]);
    assert_eq!(
        store.get_route("route-2").unwrap().state,
        RouteState::Closed
    );
}

#[test]
fn route_reconcile_detects_running_orphan_conflict_and_stale_snapshot() {
    let store = InMemoryGuardStore::default();
    let owner = register_stream(&store, "stream-a", "inst-a");
    let routes = RouteService::new(store.clone());
    routes
        .create_allocated(RouteRecord {
            route_id: "route-1".to_string(),
            resource_id: "res-1".to_string(),
            node_id: owner.node_id.clone(),
            instance_id: owner.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })
        .unwrap();
    routes
        .create_allocated(RouteRecord {
            route_id: "route-2".to_string(),
            resource_id: "res-2".to_string(),
            node_id: owner.node_id.clone(),
            instance_id: owner.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })
        .unwrap();

    let report = routes
        .apply_snapshot(ResourceSnapshot {
            owner: owner.clone(),
            generation: 1,
            sequence: 1,
            full: true,
            resources: vec![SnapshotResource {
                resource_id: "res-1".to_string(),
                resource_type: "stream".to_string(),
                route_id: Some("route-1".to_string()),
                lease_id: Some("lease-1".to_string()),
                route_state: RouteState::Running,
                endpoints: Vec::new(),
            }],
        })
        .unwrap();
    assert!(report.issues.contains(&RecoveryIssue::Orphan {
        resource_id: "res-2".to_string(),
        node_id: "stream-a".to_string()
    }));

    let stale = routes
        .apply_snapshot(ResourceSnapshot {
            owner,
            generation: 1,
            sequence: 1,
            full: true,
            resources: vec![],
        })
        .unwrap();
    assert!(
        stale
            .issues
            .iter()
            .any(|issue| matches!(issue, RecoveryIssue::StaleSnapshot { .. }))
    );
}
