use super::*;
#[cfg(unix)]
use base::net::{
    transport::MessageTransport,
    uds::{ManagedUnixStreamListener, UnixTransportConfig},
};
use gmv_nodec::component_management::{ComponentDrainOwner, ManagedDrainOwner};
use gmv_protocol::component_management::v1::{
    ComponentHealthState, ComponentProbeRequest, ComponentReadinessState,
};
#[cfg(unix)]
use gmv_protocol::session::v1::{ReadGrantedImageRequest, ReadGrantedImageResponse};
use gmv_protocol::{
    avai::v1::{ImageMetadata, ImageUrlSource, OwnedImageRef, SourceSpec, source_spec},
    common::v1::{
        AccessGrant, DataEndpoint, NodeKind, OperationRef, ResourceRef, TransportCapabilities,
        TransportMode,
    },
    component_management::v1::{
        AbortUpgradeRequest, ComponentAbortOutcome, ComponentDrainOutcome, ComponentOwnerState,
        DrainRequest, PrepareForUpgradeRequest,
    },
};
use std::{io::Write, time::Duration};

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

fn test_identity() -> NodeIdentity {
    NodeIdentity {
        node_id: "avai-test".to_string(),
        instance_id: "instance-test".to_string(),
        kind: NodeKind::Avai as i32,
    }
}

fn test_request_with_source(task_id: &str, source: SourceSpec) -> CreateTaskRequest {
    CreateTaskRequest {
        operation: Some(OperationRef {
            operation_id: format!("operation-{task_id}"),
            idempotency_key: format!("idempotency-{task_id}"),
        }),
        task_id: task_id.to_string(),
        capability: BUILTIN_CAPABILITY.to_string(),
        expected_avai: Some(test_identity()),
        source: Some(source),
        ..Default::default()
    }
}

fn test_request(task_id: &str) -> CreateTaskRequest {
    test_request_with_source(
        task_id,
        SourceSpec {
            source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
                url: "http://127.0.0.1/image.png".to_string(),
                expected: None,
                max_bytes: 1024,
            })),
        },
    )
}

async fn test_manager() -> (TaskManager, PathBuf) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-task-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-task-test-{id}"
    )))
    .unwrap();
    let manager = TaskManager::open(
        test_identity(),
        vec![BUILTIN_CAPABILITY.to_string()],
        TaskManagerConfig {
            database_path: root.join("avai.db"),
            worker_count: 1,
            queue_size: 8,
            max_result_bytes: 1024,
            source_policy: SourcePolicy {
                allow_private_image_urls: false,
                ..SourcePolicy::default()
            },
        },
        &runtime,
    )
    .await
    .unwrap();
    (manager, root)
}

#[tokio::test]
async fn avai_probe_uses_worker_liveness_and_admission_without_business_db_queries() {
    let (manager, root) = test_manager().await;
    let owner = ManagedDrainOwner::new("avai", Arc::new(AvaiDrainBehavior(manager.clone())));
    let request = || ComponentProbeRequest {
        operation_id: "probe-avai".into(),
        component_id: "avai".into(),
        readiness_contract_version: 1,
        deadline_epoch_ms: now_epoch_ms() + 5_000,
    };
    let starting = owner.probe(request()).await;
    assert_eq!(
        starting.readiness_state,
        ComponentReadinessState::NotReady as i32
    );
    manager.mark_runtime_ready();
    let ready = owner.probe(request()).await;
    assert_eq!(ready.readiness_state, ComponentReadinessState::Ready as i32);
    assert_eq!(ready.health_state, ComponentHealthState::Healthy as i32);
    manager.close_upgrade_admission();
    let drained = owner.probe(request()).await;
    assert_eq!(
        drained.readiness_state,
        ComponentReadinessState::NotReady as i32
    );
    assert_eq!(drained.health_state, ComponentHealthState::Healthy as i32);
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn png_bytes() -> Vec<u8> {
    use base::base64::Engine;
    base::base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
}

fn metadata(bytes: &[u8]) -> ImageMetadata {
    ImageMetadata {
        content_type: "image/png".to_string(),
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        width: 1,
        height: 1,
    }
}

async fn wait_terminal(manager: &TaskManager, task_id: &str) -> QueryTaskResponse {
    for _ in 0..100 {
        let query = manager
            .query_task(QueryTaskRequest {
                task_id: task_id.to_string(),
            })
            .await;
        if matches!(
            AiTaskState::try_from(query.state),
            Ok(AiTaskState::Succeeded | AiTaskState::Failed | AiTaskState::Cancelled)
        ) {
            return query;
        }
        base::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("task did not reach a terminal state: {task_id}");
}

#[test]
fn configured_capability_requires_an_installed_provider_manifest() {
    let error = ProviderRegistry::new(&["asset.damage.detect".to_string()], None, 1024)
        .err()
        .unwrap();
    assert_eq!(error.code, "invalid_task_config");
}

async fn serve_image_once(bytes: Vec<u8>) -> (String, base::tokio::task::JoinHandle<()>) {
    use base::tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let handle = base::tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&bytes).await.unwrap();
    });
    (format!("http://{address}/image.png"), handle)
}

#[cfg(unix)]
async fn serve_uds_image_once(
    root: &Path,
    bytes: Vec<u8>,
    runtime: &GlobalRuntime,
) -> (String, base::tokio::task::JoinHandle<()>) {
    let socket_path = root.join("session-image.sock");
    let mut config = UnixTransportConfig::new(root, &socket_path);
    config.max_message_size = 1024;
    let listener = ManagedUnixStreamListener::bind(config).await.unwrap();
    let task_runtime = runtime.clone();
    let handle = base::tokio::spawn(async move {
        let (connection, _) = listener
            .accept(&task_runtime, "avai-source-test-uds-io")
            .await
            .unwrap();
        let request = connection.receive().await.unwrap();
        let request = ReadGrantedImageRequest::decode(request.payload).unwrap();
        assert_eq!(request.grant_id, "grant-1");
        assert_eq!(request.proof, vec![1, 2, 3]);
        assert_eq!(request.image_id, "snapshot-uds-1");
        let response = ReadGrantedImageResponse {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            image: bytes,
            content_type: "image/png".to_string(),
            error: None,
        };
        connection
            .send(base::bytes::Bytes::from(response.encode_to_vec()))
            .await
            .unwrap();
        let _ = base::tokio::time::timeout(std::time::Duration::from_secs(1), connection.receive())
            .await;
        connection.close_and_wait().await.unwrap();
        listener.close_and_wait().await.unwrap();
    });
    (format!("unix://{}", socket_path.display()), handle)
}

#[tokio::test]
async fn create_is_idempotent_and_conflicting_request_is_rejected() {
    let (manager, root) = test_manager().await;
    let request = test_request("task-1");
    let first = manager.create_task(request.clone(), now_epoch_ms()).await;
    assert_eq!(first.task_id, "task-1");
    assert_eq!(first.state, AiTaskState::Pending as i32);
    let repeated = manager.create_task(request.clone(), now_epoch_ms()).await;
    assert_eq!(repeated.task_id, "task-1");

    let mut conflict = request;
    conflict.route_id = "different".to_string();
    let conflict = manager.create_task(conflict, now_epoch_ms()).await;
    assert_eq!(conflict.state, AiTaskState::Failed as i32);
    assert_eq!(conflict.error.unwrap().code, "task_conflict");
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn interrupted_running_task_is_requeued_after_repository_reopen() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-recovery-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let database_path = root.join("avai.db");
    let repository = TaskRepository::open(&database_path).await.unwrap();
    let request = test_request("task-recovery");
    repository
        .insert_or_get(&request, &request_hash(&request), 1)
        .await
        .unwrap();
    assert_eq!(
        repository
            .claim(
                "task-recovery",
                &ExecutionBinding::builtin(BuiltinImageMetadataProvider.manifest())
                    .encode()
                    .unwrap(),
                2,
            )
            .await
            .unwrap()
            .unwrap()
            .state,
        AiTaskState::Running
    );
    repository.pool.close().await;

    let reopened = TaskRepository::open(&database_path).await.unwrap();
    reopened.recover_interrupted().await.unwrap();
    assert_eq!(
        reopened.get("task-recovery").await.unwrap().unwrap().state,
        AiTaskState::Pending
    );
    assert_eq!(
        reopened.pending_task_ids().await.unwrap(),
        vec!["task-recovery".to_string()]
    );
    reopened.pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn upgrade_admission_rejects_new_tasks_without_closing_workers() {
    let (manager, root) = test_manager().await;
    manager.close_upgrade_admission();
    let response = manager
        .create_task(test_request("task-after-drain"), now_epoch_ms())
        .await;
    assert_eq!(response.state, AiTaskState::Failed as i32);
    assert_eq!(response.error.unwrap().code, "component_draining");
    assert!(manager.is_upgrade_drained().await.unwrap());
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn admitted_create_cannot_cross_terminal_upgrade_drain() {
    let (manager, root) = test_manager().await;
    let entered = Arc::new(base::tokio::sync::Semaphore::new(0));
    let release = Arc::new(base::tokio::sync::Semaphore::new(0));
    *manager
        .admission_pause
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(AdmissionPause {
        entered: entered.clone(),
        release: release.clone(),
    });
    let create_manager = manager.clone();
    let create = base::tokio::spawn(async move {
        create_manager
            .create_task(test_request("task-admitted-before-drain"), now_epoch_ms())
            .await
    });
    entered.acquire().await.unwrap().forget();
    assert_eq!(manager.in_flight_upgrade_admissions(), 1);

    let owner = ManagedDrainOwner::new("avai", Arc::new(AvaiDrainBehavior(manager.clone())));
    let prepared = owner
        .prepare_for_upgrade(PrepareForUpgradeRequest {
            operation_id: "op-admission-race".into(),
            component_id: "avai".into(),
            deadline_epoch_ms: now_epoch_ms() + 10_000,
        })
        .await;
    assert_eq!(prepared.owner_state, ComponentOwnerState::Draining as i32);
    assert_eq!(prepared.outcome, ComponentDrainOutcome::Accepted as i32);
    assert!(!manager.is_upgrade_drained().await.unwrap());

    let rejected = manager
        .create_task(test_request("task-after-close"), now_epoch_ms())
        .await;
    assert_eq!(rejected.error.unwrap().code, "component_draining");

    release.add_permits(1);
    let created = create.await.unwrap();
    assert_eq!(created.task_id, "task-admitted-before-drain");
    let drained = owner
        .drain(DrainRequest {
            operation_id: "op-admission-race".into(),
            component_id: "avai".into(),
            deadline_epoch_ms: now_epoch_ms() + 10_000,
        })
        .await;
    assert_eq!(drained.owner_state, ComponentOwnerState::Drained as i32);
    assert_eq!(manager.in_flight_upgrade_admissions(), 0);
    assert!(manager.is_upgrade_drained().await.unwrap());
    assert_eq!(
        manager
            .create_task(test_request("task-after-drained"), now_epoch_ms())
            .await
            .error
            .unwrap()
            .code,
        "component_draining"
    );
    let aborted = owner
        .abort_upgrade(AbortUpgradeRequest {
            operation_id: "op-admission-race".into(),
            component_id: "avai".into(),
            deadline_epoch_ms: now_epoch_ms() + 10_000,
        })
        .await;
    assert_eq!(aborted.owner_state, ComponentOwnerState::Accepting as i32);
    assert_eq!(aborted.outcome, ComponentAbortOutcome::Resumed as i32);
    assert!(manager.admission.acquire().is_some());
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn concurrent_idempotent_inserts_resolve_to_one_durable_task() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "avai-concurrent-create-test-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let repository = TaskRepository::open(&root.join("avai.db")).await.unwrap();
    let request = test_request("task-concurrent");
    let hash = request_hash(&request);
    let (first, second) = base::tokio::join!(
        repository.insert_or_get(&request, &hash, 1),
        repository.insert_or_get(&request, &hash, 1)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.created, second.created);
    assert_eq!(first.record.task_id, second.record.task_id);
    assert_eq!(repository.list().await.unwrap().len(), 1);
    repository.pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn recovery_feeder_does_not_block_startup_when_the_queue_is_full() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-feeder-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let database_path = root.join("avai.db");
    let repository = TaskRepository::open(&database_path).await.unwrap();
    for index in 0..4 {
        let request = test_request_with_source(
            &format!("task-feeder-{index}"),
            SourceSpec {
                source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
                    url: format!("http://{address}/image.png"),
                    expected: None,
                    max_bytes: 1024,
                })),
            },
        );
        repository
            .insert_or_get(&request, &request_hash(&request), 1)
            .await
            .unwrap();
    }
    repository.pool.close().await;
    let server_cancel = CancellationToken::new();
    let wait_cancel = server_cancel.clone();
    let server = base::tokio::spawn(async move {
        let mut connections = Vec::new();
        loop {
            base::tokio::select! {
                _ = wait_cancel.cancelled() => return,
                accepted = listener.accept() => connections.push(accepted.unwrap().0),
            }
        }
    });
    let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-feeder-test-{id}"
    )))
    .unwrap();
    let manager = base::tokio::time::timeout(
        Duration::from_millis(500),
        TaskManager::open(
            test_identity(),
            vec![BUILTIN_CAPABILITY.to_string()],
            TaskManagerConfig {
                database_path,
                worker_count: 1,
                queue_size: 1,
                max_result_bytes: 1024,
                source_policy: SourcePolicy {
                    allow_private_image_urls: true,
                    allowed_internal_hosts: HashSet::from(["127.0.0.1".to_string()]),
                    request_timeout: Duration::from_secs(30),
                    ..SourcePolicy::default()
                },
            },
            &runtime,
        ),
    )
    .await
    .expect("task manager startup must not wait for the recovery queue")
    .unwrap();
    manager.close_and_wait().await.unwrap();
    server_cancel.cancel();
    server.await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancel_and_success_race_selects_one_immutable_terminal_state() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-race-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let repository = TaskRepository::open(&root.join("avai.db")).await.unwrap();
    let request = test_request("task-race");
    repository
        .insert_or_get(&request, &request_hash(&request), 1)
        .await
        .unwrap();
    repository
        .claim(
            "task-race",
            &ExecutionBinding::builtin(BuiltinImageMetadataProvider.manifest())
                .encode()
                .unwrap(),
            2,
        )
        .await
        .unwrap()
        .unwrap();
    let (cancelled, succeeded) = base::tokio::join!(
        repository.cancel("task-race", 3),
        repository.succeed(
            "task-race",
            InferenceOutput {
                result: AiTaskResult::default(),
            },
            3,
        )
    );
    cancelled.unwrap();
    succeeded.unwrap();
    let terminal = repository.get("task-race").await.unwrap().unwrap().state;
    assert!(matches!(
        terminal,
        AiTaskState::Succeeded | AiTaskState::Cancelled
    ));

    repository.cancel("task-race", 4).await.unwrap();
    repository
        .succeed(
            "task-race",
            InferenceOutput {
                result: AiTaskResult::default(),
            },
            4,
        )
        .await
        .unwrap();
    assert_eq!(
        repository.get("task-race").await.unwrap().unwrap().state,
        terminal
    );
    repository.pool.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn full_queue_fails_the_new_task_without_unbounded_waiting() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-queue-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-queue-test-{id}"
    )))
    .unwrap();
    let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server_cancel = CancellationToken::new();
    let wait_cancel = server_cancel.clone();
    let server = base::tokio::spawn(async move {
        let _connection = listener.accept().await.unwrap();
        wait_cancel.cancelled().await;
    });
    let manager = TaskManager::open(
        test_identity(),
        vec![BUILTIN_CAPABILITY.to_string()],
        TaskManagerConfig {
            database_path: root.join("avai.db"),
            worker_count: 1,
            queue_size: 1,
            max_result_bytes: 1024,
            source_policy: SourcePolicy {
                allow_private_image_urls: true,
                allowed_internal_hosts: HashSet::from(["127.0.0.1".to_string()]),
                request_timeout: Duration::from_secs(30),
                ..SourcePolicy::default()
            },
        },
        &runtime,
    )
    .await
    .unwrap();
    let blocking_request = |task_id: &str| {
        test_request_with_source(
            task_id,
            SourceSpec {
                source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
                    url: format!("http://{address}/image.png"),
                    expected: None,
                    max_bytes: 1024,
                })),
            },
        )
    };
    manager
        .create_task(blocking_request("task-running"), now_epoch_ms())
        .await;
    for _ in 0..100 {
        if manager.running_task_count() == 1 {
            break;
        }
        base::tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(manager.running_task_count(), 1);
    let mut queued = blocking_request("task-queued");
    queued.deadline_epoch_ms = now_epoch_ms() + 20;
    manager.create_task(queued, now_epoch_ms()).await;
    let rejected = manager
        .create_task(blocking_request("task-rejected"), now_epoch_ms())
        .await;
    assert_eq!(rejected.state, AiTaskState::Failed as i32);
    assert_eq!(rejected.error.unwrap().code, "resource_exhausted");

    base::tokio::time::sleep(Duration::from_millis(30)).await;
    let cancelled = manager
        .cancel_task(CancelTaskRequest {
            task_id: "task-running".to_string(),
            ..Default::default()
        })
        .await;
    assert_eq!(cancelled.state, AiTaskState::Cancelled as i32);
    let queued = wait_terminal(&manager, "task-queued").await;
    assert_eq!(queued.state, AiTaskState::Failed as i32);
    assert_eq!(queued.error.unwrap().code, "task_expired");
    manager.close_and_wait().await.unwrap();
    server_cancel.cancel();
    server.await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stream_source_has_a_stable_rejection() {
    let (manager, root) = test_manager().await;
    let mut request = test_request("task-stream");
    request.source = Some(SourceSpec {
        source: Some(source_spec::Source::StreamFrame(Default::default())),
    });
    let response = manager.create_task(request, now_epoch_ms()).await;
    assert_eq!(response.state, AiTaskState::Pending as i32);
    for _ in 0..50 {
        let query = manager
            .query_task(QueryTaskRequest {
                task_id: "task-stream".to_string(),
            })
            .await;
        if query.state == AiTaskState::Failed as i32 {
            assert_eq!(query.error.unwrap().code, "source_transport_unsupported");
            manager.close_and_wait().await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        base::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("stream source task did not reach a terminal state");
}

#[tokio::test]
async fn local_object_url_and_session_owned_sources_share_the_same_pipeline() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-source-test-{}-{id}", std::process::id()));
    let object_root = root.join("objects");
    std::fs::create_dir_all(&object_root).unwrap();
    let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-source-test-{id}"
    )))
    .unwrap();
    let manager = TaskManager::open(
        test_identity(),
        vec![BUILTIN_CAPABILITY.to_string()],
        TaskManagerConfig {
            database_path: root.join("avai.db"),
            worker_count: 2,
            queue_size: 8,
            max_result_bytes: 1024,
            source_policy: SourcePolicy {
                allow_private_image_urls: true,
                allowed_internal_hosts: HashSet::from(["127.0.0.1".to_string()]),
                object_root: object_root.clone(),
                uds_socket_root: root.join("run"),
                ..SourcePolicy::default()
            },
        },
        &runtime,
    )
    .await
    .unwrap();
    let bytes = png_bytes();
    let mut object = std::fs::File::create(object_root.join("object-1")).unwrap();
    object.write_all(&bytes).unwrap();
    drop(object);
    let grant = |endpoint: String| AccessGrant {
        grant_id: "grant-1".to_string(),
        expected_consumer: Some(test_identity()),
        purpose: BUILTIN_CAPABILITY.to_string(),
        expires_at_epoch_ms: now_epoch_ms() + 60_000,
        endpoints: vec![DataEndpoint {
            name: "image".to_string(),
            uri: endpoint,
            capabilities: Some(TransportCapabilities {
                reliable: true,
                ordered: true,
                preserves_message_boundary: false,
                encrypted: false,
                congestion_controlled: true,
                local_only: true,
                max_message_size: 1024,
                mode: TransportMode::Stream as i32,
            }),
            labels: HashMap::new(),
        }],
        proof: vec![1, 2, 3],
    };
    let local_source = SourceSpec {
        source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
            owner: Some(test_identity()),
            resource: Some(ResourceRef {
                resource_id: "object-1".to_string(),
                resource_type: "avai_image".to_string(),
            }),
            metadata: Some(metadata(&bytes)),
            access: Some(grant("gmv-object://object-1".to_string())),
        })),
    };
    let response = manager
        .create_task(
            test_request_with_source("task-object", local_source),
            now_epoch_ms(),
        )
        .await;
    assert_eq!(response.state, AiTaskState::Pending as i32);
    assert_eq!(
        wait_terminal(&manager, "task-object").await.state,
        AiTaskState::Succeeded as i32
    );

    let (url, url_server) = serve_image_once(bytes.clone()).await;
    let url_source = SourceSpec {
        source: Some(source_spec::Source::ImageUrl(ImageUrlSource {
            url,
            expected: Some(metadata(&bytes)),
            max_bytes: 1024,
        })),
    };
    manager
        .create_task(
            test_request_with_source("task-url", url_source),
            now_epoch_ms(),
        )
        .await;
    assert_eq!(
        wait_terminal(&manager, "task-url").await.state,
        AiTaskState::Succeeded as i32
    );
    url_server.await.unwrap();

    let (session_url, session_server) = serve_image_once(bytes.clone()).await;
    let session_source = SourceSpec {
        source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
            owner: Some(NodeIdentity {
                node_id: "session-1".to_string(),
                instance_id: "session-instance-1".to_string(),
                kind: NodeKind::Session as i32,
            }),
            resource: Some(ResourceRef {
                resource_id: "snapshot-1".to_string(),
                resource_type: "gb28181_image".to_string(),
            }),
            metadata: Some(metadata(&bytes)),
            access: Some(grant(session_url)),
        })),
    };
    manager
        .create_task(
            test_request_with_source("task-session", session_source),
            now_epoch_ms(),
        )
        .await;
    let session_result = wait_terminal(&manager, "task-session").await;
    assert_eq!(session_result.state, AiTaskState::Succeeded as i32);
    assert_eq!(
        session_result
            .typed_result
            .unwrap()
            .actual_model
            .unwrap()
            .model_id,
        "builtin.image-metadata"
    );
    session_server.await.unwrap();

    #[cfg(unix)]
    {
        let (session_uds, session_uds_server) =
            serve_uds_image_once(&root.join("run"), bytes.clone(), &runtime).await;
        let session_uds_source = SourceSpec {
            source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                owner: Some(NodeIdentity {
                    node_id: "session-1".to_string(),
                    instance_id: "session-instance-1".to_string(),
                    kind: NodeKind::Session as i32,
                }),
                resource: Some(ResourceRef {
                    resource_id: "snapshot-uds-1".to_string(),
                    resource_type: "gb28181_image".to_string(),
                }),
                metadata: Some(metadata(&bytes)),
                access: Some(grant(session_uds)),
            })),
        };
        manager
            .create_task(
                test_request_with_source("task-session-uds", session_uds_source),
                now_epoch_ms(),
            )
            .await;
        assert_eq!(
            wait_terminal(&manager, "task-session-uds").await.state,
            AiTaskState::Succeeded as i32
        );
        session_uds_server.await.unwrap();
    }

    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
