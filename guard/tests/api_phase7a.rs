use guard::api::v2::paths;
use guard::api::v2::{ApiV2, CursorQuery, EventQuery};
use guard::auth::Role;
use guard::core::GuardError;
use guard::job::{SystemJobRequest, SystemJobService, SystemJobStatus, SystemJobType};
use guard::operation::{OperationRequest, OperationService, OperationStatus};
use guard::store::InMemoryGuardStore;
use guard::store::model::EventRecord;

#[test]
fn api_v2_paths_are_rest_polling_first() {
    assert_eq!(paths::API_PREFIX, "/api/v2");
    assert_eq!(paths::LEASES, "/api/v2/leases");
    assert_eq!(paths::EVENTS, "/api/v2/events");
    assert_eq!(paths::SSE_EVENTS_STREAM, "/api/v2/events/stream");
    assert!(paths::is_v2_path("/api/v2/events"));
    assert!(!paths::is_v2_path("/ws/v2/events"));
}

#[test]
fn event_cursor_polling_returns_incremental_pages() {
    let store = InMemoryGuardStore::default();
    for (event_id, topic, priority) in [
        ("0001", "node.stream.health", 1),
        ("0002", "route.stream.running", 2),
        ("0003", "node.session.health", 1),
    ] {
        store
            .insert_event_once(EventRecord {
                event_id: event_id.to_string(),
                topic: topic.to_string(),
                priority,
                payload: vec![],
            })
            .unwrap();
    }

    let page = ApiV2::new(
        store,
        OperationService::default(),
        SystemJobService::default(),
    )
    .poll_events(EventQuery {
        cursor: CursorQuery {
            after_id: Some("0001".to_string()),
            limit: 1,
        },
        topic_prefix: None,
        min_priority: None,
    })
    .unwrap();

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].event_id, "0002");
    assert_eq!(page.next_after_id.as_deref(), Some("0002"));
}

#[test]
fn event_cursor_polling_validates_page_size() {
    let api = ApiV2::new(
        InMemoryGuardStore::default(),
        OperationService::default(),
        SystemJobService::default(),
    );
    let err = api
        .poll_events(EventQuery {
            cursor: CursorQuery {
                after_id: None,
                limit: 0,
            },
            topic_prefix: None,
            min_priority: None,
        })
        .unwrap_err();
    assert!(matches!(err, GuardError::InvalidConfig(_)));
}

#[test]
fn operation_requires_role_and_confirmation_for_dangerous_actions() {
    let operations = OperationService::default();
    assert!(
        operations
            .start(OperationRequest {
                operation_id: "op-1".to_string(),
                kind: "node.takeover".to_string(),
                requested_by: "operator".to_string(),
                caller_role: Role::Viewer,
                required_role: Role::Operator,
                dangerous: false,
                confirmation: None,
            })
            .is_err()
    );
    assert!(
        operations
            .start(OperationRequest {
                operation_id: "op-1".to_string(),
                kind: "node.takeover".to_string(),
                requested_by: "operator".to_string(),
                caller_role: Role::Operator,
                required_role: Role::Operator,
                dangerous: true,
                confirmation: None,
            })
            .is_err()
    );

    let record = operations
        .start(OperationRequest {
            operation_id: "op-1".to_string(),
            kind: "node.takeover".to_string(),
            requested_by: "operator".to_string(),
            caller_role: Role::Operator,
            required_role: Role::Operator,
            dangerous: true,
            confirmation: Some("node.takeover".to_string()),
        })
        .unwrap();
    assert_eq!(record.status, OperationStatus::Accepted);
    assert_eq!(
        operations.progress("op-1", 50, "half").unwrap().status,
        OperationStatus::Running
    );
    assert_eq!(
        operations.succeed("op-1", "done").unwrap().status,
        OperationStatus::Succeeded
    );
    assert!(operations.progress("op-1", 80, "late").is_err());
}

#[test]
fn system_job_tracks_progress_and_terminal_state() {
    let jobs = SystemJobService::default();
    let started = jobs
        .start(SystemJobRequest {
            job_id: "job-1".to_string(),
            job_type: SystemJobType::Backup,
        })
        .unwrap();
    assert_eq!(started.status, SystemJobStatus::Pending);
    assert_eq!(
        jobs.progress("job-1", 10, "copying").unwrap().status,
        SystemJobStatus::Running
    );
    assert_eq!(
        jobs.succeed("job-1", "done").unwrap().status,
        SystemJobStatus::Succeeded
    );
    assert!(
        jobs.fail("job-1", GuardError::Conflict("late".to_string()))
            .is_err()
    );
}
