use guard::core::{LeaseState, NodeIdentity, NodeKind, RouteState};
use guard::gateway::{AllocationRequest, AllocationService};
use guard::lease::{LeaseRequest, LeaseService};
use guard::registry::{RegisterRequest, RegistryService};
use guard::route::{RecoveryIssue, ResourceSnapshot, RouteService, SnapshotResource};
use guard::store::InMemoryGuardStore;
use guard::store::model::RouteRecord;

fn stream_identity(node_id: &str, instance_id: &str) -> NodeIdentity {
    NodeIdentity::new(node_id, instance_id, NodeKind::Stream)
}

fn register_stream(
    store: &InMemoryGuardStore,
    node_id: &str,
    instance_id: &str,
    capacity: u32,
) -> NodeIdentity {
    let identity = stream_identity(node_id, instance_id);
    RegistryService::new(store.clone())
        .register(RegisterRequest {
            identity: identity.clone(),
            capabilities: vec!["live".to_string()],
            endpoints: vec![],
            capacity,
            host_metrics: Default::default(),
            zone: Some("z1".to_string()),
            now_ms: 1_000,
            takeover: false,
        })
        .unwrap();
    identity
}

#[test]
fn allocation_filters_scores_and_explains_selection() {
    let store = InMemoryGuardStore::default();
    let left = register_stream(&store, "stream-a", "inst-a", 1);
    let right = register_stream(&store, "stream-b", "inst-b", 4);
    let result = AllocationService::new(store)
        .allocate(AllocationRequest {
            request_id: "req-1".to_string(),
            capability: "live".to_string(),
            zone: Some("z1".to_string()),
        })
        .unwrap();

    assert_eq!(result.owner, right);
    assert_eq!(result.explain.selected_node_id, "stream-b");
    assert!(
        result
            .explain
            .scores
            .iter()
            .any(|score| score.node_id == left.node_id)
    );
}

#[test]
fn lease_state_machine_rejects_stale_instance_and_expires() {
    let store = InMemoryGuardStore::default();
    let owner = register_stream(&store, "stream-a", "inst-a", 1);
    let service = LeaseService::new(store.clone());
    service
        .allocate(LeaseRequest {
            lease_id: "lease-1".to_string(),
            route_id: "route-1".to_string(),
            resource_id: "stream-001".to_string(),
            idempotency_key: "idem-1".to_string(),
            owner: owner.clone(),
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
            idempotency_key: "idem-2".to_string(),
            owner,
            now_ms: 1_000,
            ttl_ms: 10,
        })
        .unwrap();
    assert_eq!(service.expire_due(1_011), vec!["lease-2".to_string()]);
}

#[test]
fn route_reconcile_detects_running_orphan_conflict_and_stale_snapshot() {
    let store = InMemoryGuardStore::default();
    let owner = register_stream(&store, "stream-a", "inst-a", 2);
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
            resources: vec![SnapshotResource {
                resource_id: "res-1".to_string(),
                route_id: Some("route-1".to_string()),
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
