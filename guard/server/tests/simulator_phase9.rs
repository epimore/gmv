use guard::core::{ClockState, HealthState, LeaseState, SchedulingState};
use guard::sim::{EndpointMode, SimAiTaskState, SimFaults, SimStreamState, Simulator};
use guard::store::InMemoryGuardStore;

#[test]
fn simulator_registers_three_node_types_and_handles_health_clock_capability() {
    let simulator = Simulator::new(InMemoryGuardStore::default(), EndpointMode::Multi);
    simulator.bootstrap(1_000).unwrap();
    assert_eq!(simulator.store().nodes().len(), 3);
    simulator
        .heartbeat("stream-sim-1", HealthState::Degraded, 1, 1_100)
        .unwrap();
    assert_eq!(
        simulator
            .store()
            .get_node("stream-sim-1")
            .unwrap()
            .scheduling,
        SchedulingState::Disabled
    );
    simulator
        .heartbeat("stream-sim-1", HealthState::Ready, 2, 1_200)
        .unwrap();
    assert_eq!(
        simulator.set_clock_offset("stream-sim-1", 5_000).unwrap(),
        ClockState::TimeUnsynced
    );
    assert_eq!(
        simulator
            .store()
            .get_node("stream-sim-1")
            .unwrap()
            .scheduling,
        SchedulingState::TimeUnsynced
    );
    simulator.set_clock_offset("stream-sim-1", 10).unwrap();
    simulator
        .set_capabilities("stream-sim-1", vec!["live".to_string()])
        .unwrap();
}

#[test]
fn simulator_completes_stream_ptz_ai_and_failure_paths() {
    let simulator = Simulator::new(InMemoryGuardStore::default(), EndpointMode::Multi);
    simulator.bootstrap(1_000).unwrap();
    let stream = simulator
        .start_stream("req-1", "34020000001320000001", "ch-1", 2_000)
        .unwrap();
    assert_eq!(stream.state, SimStreamState::Running);
    assert!(stream.endpoint.contains(','));
    assert_eq!(
        simulator.lease_state(&stream.lease_id),
        Some(LeaseState::Confirmed)
    );
    assert_eq!(
        simulator
            .ptz(&stream.device_id, &stream.channel_id)
            .unwrap(),
        1
    );
    let task = simulator
        .start_ai("req-ai-1", &stream.stream_id, "vehicle", 2_100)
        .unwrap();
    assert_eq!(task.state, SimAiTaskState::Running);
    assert_eq!(
        simulator.cancel_ai(&task.task_id).unwrap().state,
        SimAiTaskState::Cancelled
    );
    assert_eq!(
        simulator.stop_stream(&stream.stream_id).unwrap().state,
        SimStreamState::Stopped
    );

    simulator.set_faults(SimFaults {
        fail_next_stream_start: true,
        fail_next_ai_start: false,
    });
    assert!(
        simulator
            .start_stream("req-fail", "34020000001320000001", "ch-1", 3_000)
            .is_err()
    );
    assert_eq!(
        simulator.lease_state("lease-req-fail"),
        Some(LeaseState::Failed)
    );
}

#[test]
fn guard_interruption_preserves_existing_resources_and_reconcile_recovers() {
    let simulator = Simulator::new(InMemoryGuardStore::default(), EndpointMode::Single);
    simulator.bootstrap(1_000).unwrap();
    let stream = simulator
        .start_stream("req-1", "34020000001320000001", "ch-1", 2_000)
        .unwrap();
    let task = simulator
        .start_ai("req-ai-1", &stream.stream_id, "vehicle", 2_100)
        .unwrap();
    simulator.set_guard_available(false);
    assert!(
        simulator
            .start_stream("req-2", "34020000001320000001", "ch-2", 2_200)
            .is_err()
    );
    let status = simulator.status();
    assert_eq!(status.running_streams, 1);
    assert_eq!(status.running_ai_tasks, 1);
    simulator.set_guard_available(true);
    assert!(simulator.reconcile().unwrap().is_empty());
    assert_eq!(simulator.streams()[0].stream_id, stream.stream_id);
    assert_eq!(simulator.ai_tasks()[0].task_id, task.task_id);
}
