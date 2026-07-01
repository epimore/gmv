use std::collections::HashMap;

use guard::bus::{BusEvent, BusPriority, BusService};
use guard::core::{
    ClockClassifier, ClockState, ConnectionState, HealthState, NodeIdentity, NodeKind,
    SchedulingState, generate_instance_id,
};
use guard::registry::{
    AllowedNode, HeartbeatReport, RegisterDecision, RegisterRequest, RegistryPolicy,
    RegistryService,
};
use guard::store::InMemoryGuardStore;

fn identity(node_id: &str, instance_id: &str) -> NodeIdentity {
    NodeIdentity::new(node_id, instance_id, NodeKind::Stream)
}

#[test]
fn registry_policy_seeds_offline_allowed_nodes() {
    let store = InMemoryGuardStore::default();
    let registry = RegistryService::with_policy(
        store.clone(),
        RegistryPolicy {
            allow_unknown_nodes: false,
            allowed_nodes: HashMap::from([(
                "session-gb-1".to_string(),
                AllowedNode {
                    kind: NodeKind::Session,
                    service: "session-gb28181".to_string(),
                    required_capabilities: vec!["protocol.gb28181".to_string()],
                },
            )]),
        },
    );

    let node = store.get_node("session-gb-1").unwrap();
    assert_eq!(node.connection, ConnectionState::Disconnected);
    assert_eq!(node.health, HealthState::Offline);
    assert_eq!(node.scheduling, SchedulingState::Disabled);
    assert_eq!(node.capacity, 0);
    assert_eq!(
        node.config.get("service").map(String::as_str),
        Some("session-gb28181")
    );

    assert_eq!(
        registry
            .register(RegisterRequest {
                identity: NodeIdentity::new("session-gb-1", "inst-1", NodeKind::Session),
                capabilities: vec!["protocol.gb28181".to_string()],
                endpoints: vec![],
                capacity: 1,
                host_metrics: Default::default(),
                zone: None,
                now_ms: 1_000,
                takeover: false,
                config: Default::default(),
            })
            .unwrap(),
        RegisterDecision::Accepted
    );
    let node = store.get_node("session-gb-1").unwrap();
    assert_eq!(node.connection, ConnectionState::Connected);
    assert_eq!(
        node.config.get("service").map(String::as_str),
        Some("session-gb28181")
    );
}

#[test]
fn registry_fences_old_instances_and_sequences() {
    let store = InMemoryGuardStore::default();
    let registry = RegistryService::new(store);
    let first = identity("stream-1", &generate_instance_id());
    let second = identity("stream-1", &generate_instance_id());

    assert_eq!(
        registry
            .register(RegisterRequest {
                identity: first.clone(),
                capabilities: vec!["live".to_string()],
                endpoints: vec![],
                capacity: 2,
                host_metrics: Default::default(),
                zone: None,
                now_ms: 1_000,
                takeover: false,
                config: Default::default(),
            })
            .unwrap(),
        RegisterDecision::Accepted
    );
    assert!(
        registry
            .register(RegisterRequest {
                identity: second.clone(),
                capabilities: vec!["live".to_string()],
                endpoints: vec![],
                capacity: 2,
                host_metrics: Default::default(),
                zone: None,
                now_ms: 1_001,
                takeover: false,
                config: Default::default(),
            })
            .is_err()
    );
    assert_eq!(
        registry
            .register(RegisterRequest {
                identity: second.clone(),
                capabilities: vec!["live".to_string()],
                endpoints: vec![],
                capacity: 2,
                host_metrics: Default::default(),
                zone: None,
                now_ms: 1_002,
                takeover: true,
                config: Default::default(),
            })
            .unwrap(),
        RegisterDecision::SupersededOldInstance
    );
    assert!(
        registry
            .heartbeat(HeartbeatReport {
                identity: first,
                health: HealthState::Ready,
                sequence: 1,
                now_ms: 1_003,
                host_metrics: Default::default(),
                business_metrics: Default::default(),
            })
            .is_err()
    );
    registry
        .heartbeat(HeartbeatReport {
            identity: second.clone(),
            health: HealthState::Ready,
            sequence: 1,
            now_ms: 1_004,
            host_metrics: Default::default(),
            business_metrics: Default::default(),
        })
        .unwrap();
    assert!(
        registry
            .heartbeat(HeartbeatReport {
                identity: second,
                health: HealthState::Ready,
                sequence: 1,
                now_ms: 1_005,
                host_metrics: Default::default(),
                business_metrics: Default::default(),
            })
            .is_err()
    );
}

#[test]
fn time_offset_classification_blocks_severe_drift() {
    let classifier = ClockClassifier::default();
    assert_eq!(classifier.classify(100), ClockState::Synced);
    assert_eq!(classifier.classify(1_500), ClockState::Warn);
    assert_eq!(classifier.classify(-5_000), ClockState::TimeUnsynced);
}

#[test]
fn bus_isolates_slow_consumers_and_deduplicates_events() {
    let bus = BusService::new(InMemoryGuardStore::default());
    let slow = bus.subscribe("node.**", 1).unwrap();
    let fast = bus.subscribe("node.*.health", 8).unwrap();

    bus.publish(BusEvent {
        event_id: "e1".to_string(),
        topic: "node.stream.health".to_string(),
        priority: BusPriority::P2,
        payload: vec![1],
    })
    .unwrap();
    bus.publish(BusEvent {
        event_id: "e2".to_string(),
        topic: "node.stream.health".to_string(),
        priority: BusPriority::P2,
        payload: vec![2],
    })
    .unwrap();

    assert_eq!(bus.queue_len(slow).unwrap(), 1);
    assert_eq!(bus.queue_len(fast).unwrap(), 2);
    let duplicate = bus
        .publish(BusEvent {
            event_id: "e2".to_string(),
            topic: "node.stream.health".to_string(),
            priority: BusPriority::P1,
            payload: vec![2],
        })
        .unwrap();
    assert!(duplicate.duplicate);
}
