use super::*;

#[test]
fn avai_registers_filters_capabilities_and_handles_idempotent_tasks() {
    let node = AvaiGuardNode::new(
        "avai-1",
        "inst-1",
        "127.0.0.1",
        "http://127.0.0.1:18080",
        19090,
        vec!["ai.vehicle".to_string()],
    );
    let register = node.register_request(NodeResourceSnapshot {
        resources: vec![],
        full: true,
    });
    assert_eq!(register.identity.unwrap().kind, NodeKind::Avai as i32);
    assert_eq!(register.capabilities, vec!["ai.vehicle".to_string()]);
    let heartbeat = node.heartbeat_message(1, 1000, 0);
    assert!(matches!(
        heartbeat.payload,
        Some(node_to_guard_message::Payload::Heartbeat(_))
    ));

    let mut control = AvaiControlAdapter::new(node.identity.clone(), node.capabilities.clone());
    let frame = FrameReference {
        frame_ref: "frame-1".to_string(),
        stream_id: "stream-1".to_string(),
        expires_at_epoch_ms: 2000,
    };
    let request = CreateTaskRequest {
        operation: Some(operation("ai-1")),
        task_id: "task-1".to_string(),
        task_type: "ai.vehicle".to_string(),
        route_id: "route-1".to_string(),
        expected_avai: Some(node.identity.clone()),
        payload: frame.encode(),
        ..Default::default()
    };
    let response = control.create_task(request.clone(), 1000);
    assert_eq!(response.state, AiTaskState::Failed as i32);
    assert_eq!(response.error.unwrap().code, "executor_unavailable");
    let repeated = control.create_task(request, 1000);
    assert_eq!(repeated.state, AiTaskState::Failed as i32);
    assert_eq!(control.resource_snapshot().resources.len(), 0);
    assert!(control.progress_event("task-1", 50).is_none());
    assert!(control.complete_task("task-1", b"ok".to_vec()).is_none());
}

#[test]
fn avai_task_result_has_a_stable_public_payload() {
    let task = AiTask {
        task_type: "ai.vehicle".to_string(),
        route_id: "route-1".to_string(),
        frame: None,
        state: AiTaskState::Succeeded,
        result: Vec::new(),
    };
    let payload = avai_task_result_payload(
        "task-1",
        &task,
        br#"{"detections":[{"label":"car","score":0.98}]}"#,
    )
    .unwrap();
    let payload: base::serde_json::Value = base::serde_json::from_slice(&payload).unwrap();
    assert_eq!(payload["task_id"], "task-1");
    assert_eq!(payload["task_type"], "ai.vehicle");
    assert_eq!(payload["route_id"], "route-1");
    assert_eq!(payload["state"], "succeeded");
    assert_eq!(payload["result"]["detections"][0]["label"], "car");
}

#[test]
fn avai_rejects_expired_frame_unknown_model_and_stale_instance() {
    let node = AvaiGuardNode::new(
        "avai-1",
        "inst-1",
        "127.0.0.1",
        "http://127.0.0.1:18080",
        19090,
        vec!["ai.vehicle".to_string()],
    );
    let mut control = AvaiControlAdapter::new(node.identity.clone(), node.capabilities.clone());
    let expired = FrameReference {
        frame_ref: "frame-old".to_string(),
        stream_id: "stream-1".to_string(),
        expires_at_epoch_ms: 10,
    };
    let response = control.create_task(
        CreateTaskRequest {
            operation: Some(operation("expired")),
            task_id: "task-expired".to_string(),
            task_type: "ai.vehicle".to_string(),
            route_id: "route-1".to_string(),
            expected_avai: Some(node.identity.clone()),
            payload: expired.encode(),
            ..Default::default()
        },
        20,
    );
    assert_eq!(response.state, AiTaskState::Failed as i32);
    let missing = control.create_task(
        CreateTaskRequest {
            operation: Some(operation("missing")),
            task_id: "task-missing".to_string(),
            task_type: "ai.face".to_string(),
            route_id: "route-1".to_string(),
            expected_avai: Some(node.identity.clone()),
            payload: vec![],
            ..Default::default()
        },
        20,
    );
    assert_eq!(missing.state, AiTaskState::Failed as i32);
    let stale = NodeIdentity {
        node_id: "avai-1".to_string(),
        instance_id: "old".to_string(),
        kind: NodeKind::Avai as i32,
    };
    let stale_response = control.create_task(
        CreateTaskRequest {
            operation: Some(operation("stale")),
            task_id: "task-stale".to_string(),
            task_type: "ai.vehicle".to_string(),
            route_id: "route-1".to_string(),
            expected_avai: Some(stale),
            payload: vec![],
            ..Default::default()
        },
        20,
    );
    assert_eq!(stale_response.state, AiTaskState::Failed as i32);
}
