use super::*;

fn identity(index: usize) -> ActualModelIdentity {
    ActualModelIdentity {
        model_id: format!("model-{index}"),
        version: "1".to_string(),
        revision: format!("rev-{index}"),
        runtime: "fake".to_string(),
    }
}

#[test]
fn snapshot_is_fixed_key_bounded_and_process_epoch_scoped() {
    let telemetry = Observability::new_at(100);
    telemetry.set_installed_models(3);
    telemetry.set_ready_models(2);
    telemetry.observe_preload(Duration::from_millis(500));
    telemetry.observe_self_test_failure();
    telemetry.observe_activation_failure();
    telemetry.observe_task_terminal(None, TaskTerminalOutcome::Failed);
    for index in 0..ACTUAL_MODEL_CAPACITY {
        telemetry.observe_task_terminal(Some(identity(index)), TaskTerminalOutcome::Succeeded);
    }
    telemetry.observe_task_terminal(Some(identity(16)), TaskTerminalOutcome::Cancelled);
    telemetry.observe_task_terminal(Some(identity(0)), TaskTerminalOutcome::Failed);

    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot["telemetry_process_start_epoch_ms"], "100");
    assert_eq!(snapshot["installed_models"], "3");
    assert_eq!(snapshot["ready_models"], "2");
    assert_eq!(snapshot["preload_seconds_count"], "1");
    assert_eq!(snapshot["preload_seconds_le_500ms"], "1");
    assert_eq!(snapshot["preload_seconds_le_100ms"], "0");
    assert_eq!(snapshot["self_test_failures_total"], "1");
    assert_eq!(snapshot["activation_failures_total"], "1");
    assert_eq!(snapshot["tasks_without_actual_model_total"], "1");
    assert_eq!(snapshot["tasks_actual_model_overflow_total"], "1");
    assert_eq!(snapshot["tasks_actual_model_slot_00_succeeded"], "1");
    assert_eq!(snapshot["tasks_actual_model_slot_00_failed"], "1");
    assert_eq!(snapshot["tasks_actual_model_slot_15_succeeded"], "1");
    assert!(!snapshot.values().any(|value| value.contains("model-16")));
    assert!(snapshot.len() <= 17 + ACTUAL_MODEL_CAPACITY * 4);

    let restarted = Observability::new_at(200).snapshot();
    assert_eq!(restarted["telemetry_process_start_epoch_ms"], "200");
    assert_eq!(restarted["installed_models"], "0");
    assert_eq!(restarted["preload_seconds_count"], "0");
    assert!(!restarted.keys().any(|key| key.contains("slot_00")));
}
