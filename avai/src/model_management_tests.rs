use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::Arc,
};

use base::{
    sha2::{Digest, Sha256},
    tokio_util::sync::CancellationToken,
    utils::rt::{GlobalRuntime, RuntimeType},
};
use gmv_nodec::component_management::UnsupportedDrainOwner;
use gmv_protocol::{
    avai::model_management::v1::{
        ActivateModelRequest, ImportStagedModelRequest, InspectModelRequest, ListModelsRequest,
        ModelHealth, ModelIdentity as RpcIdentity, PreloadModelRequest, RollbackModelRequest,
        UnloadModelRequest, avai_model_management_client::AvaiModelManagementClient,
        avai_model_management_server::AvaiModelManagement,
    },
    common::v1::{NodeIdentity, NodeKind, OperationRef},
    component_management::v1::{
        ComponentProbeRequest, component_management_client::ComponentManagementClient,
    },
};
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};

use crate::{
    model::{
        FakeRuntimeBehavior, FakeRuntimeProvider, ModelManager, ModelManagerConfig,
        ModelRepository, RuntimeProvider, verify_package,
    },
    model_management::{AvaiModelManagementRpc, ModelManagementConfig, serve_uds},
    model_runtime_tests::{TestRoot, identity, install_test_model, policy, write_package},
    source::SourcePolicy,
    task::{TaskManager, TaskManagerConfig},
};

fn runtime(name: &str) -> GlobalRuntime {
    GlobalRuntime::register_default(RuntimeType::Custom(format!(
        "avai-model-management-{name}-{}",
        std::process::id()
    )))
    .unwrap()
}

fn node_identity() -> NodeIdentity {
    NodeIdentity {
        node_id: "avai-test".into(),
        instance_id: "instance-test".into(),
        kind: NodeKind::Avai as i32,
    }
}

async fn task_manager(root: &TestRoot, manager: ModelManager, name: &str) -> TaskManager {
    TaskManager::open_with_model_manager(
        node_identity(),
        vec![
            "image.metadata.inspect".into(),
            "vehicle.detect".into(),
            "vision.object.detect".into(),
        ],
        TaskManagerConfig {
            database_path: root.path().join(format!("tasks-{name}.db")),
            queue_size: 4,
            worker_count: 1,
            source_policy: SourcePolicy::default(),
            max_result_bytes: 1024,
        },
        Some(manager),
        &runtime(name),
    )
    .await
    .unwrap()
}

fn management_config(root: &TestRoot) -> ModelManagementConfig {
    ModelManagementConfig {
        trusted_import_root: root.path().join("import"),
        package_policy: policy(),
        receipt_capacity: 16,
        receipt_retention_ms: 24 * 60 * 60 * 1_000,
        mutation_concurrency: 1,
    }
}

fn rpc_identity(model_id: &str, version: &str, revision: &str) -> RpcIdentity {
    RpcIdentity {
        model_id: model_id.into(),
        version: version.into(),
        revision: revision.into(),
    }
}

fn operation(id: &str) -> OperationRef {
    OperationRef {
        operation_id: id.into(),
        idempotency_key: format!("key-{id}"),
    }
}

fn deadline() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60_000
}

async fn uds_channel(path: std::path::PathBuf) -> Channel {
    Endpoint::try_from(format!("unix://{}", path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

#[tokio::test]
async fn real_uds_combines_both_services_and_enforces_security_and_replay() {
    let root = TestRoot::new("management-uds");
    let import_root = root.path().join("import");
    let stage = import_root.join("stage-a");
    std::fs::create_dir_all(&stage).unwrap();
    write_package(&stage, "model-a", "1", "rev-a");
    let manifest_hash = format!(
        "{:x}",
        Sha256::digest(std::fs::read(stage.join("manifest.yaml")).unwrap())
    );

    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    let manager = ModelManager::open(
        repository.clone(),
        Vec::new(),
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let tasks = task_manager(&root, manager.clone(), "uds").await;
    let rpc = AvaiModelManagementRpc::new(
        repository.clone(),
        manager,
        tasks.clone(),
        management_config(&root),
    )
    .unwrap();
    let socket = root.path().join("management.sock");
    let cancel = CancellationToken::new();
    let server_socket = socket.clone();
    let server_cancel = cancel.clone();
    let server = base::tokio::spawn(async move {
        serve_uds(
            &server_socket,
            Arc::new(UnsupportedDrainOwner::new("avai")),
            rpc,
            server_cancel,
        )
        .await
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        base::tokio::task::yield_now().await;
    }
    assert_eq!(
        std::fs::symlink_metadata(&socket)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let channel = uds_channel(socket.clone()).await;
    let mut component = ComponentManagementClient::new(channel.clone());
    let probe = component
        .probe(ComponentProbeRequest {
            operation_id: "probe".into(),
            component_id: "avai".into(),
            readiness_contract_version: 1,
            deadline_epoch_ms: deadline(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(probe.component_id, "avai");

    let mut model = AvaiModelManagementClient::new(channel);
    let import = ImportStagedModelRequest {
        operation: Some(operation("import-a")),
        deadline_epoch_ms: deadline(),
        stage_id: "stage-a".into(),
        expected_identity: Some(rpc_identity("model-a", "1", "rev-a")),
        expected_manifest_sha256: manifest_hash,
    };
    let first = model
        .import_staged_model(import.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.error, None);
    repository
        .reset_operation_pending_for_test("import-a")
        .await
        .unwrap();
    let replay = model
        .import_staged_model(import.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(replay.replayed);
    assert_eq!(replay.error, None);
    let mut conflict = import;
    conflict.stage_id = "stage-b".into();
    assert_eq!(
        model
            .import_staged_model(conflict)
            .await
            .unwrap()
            .into_inner()
            .error
            .unwrap()
            .code,
        "model_operation_conflict"
    );

    let inspected = model
        .inspect_model(InspectModelRequest {
            identity: Some(rpc_identity("model-a", "1", "rev-a")),
            observe_live_health: false,
            deadline_epoch_ms: 0,
        })
        .await
        .unwrap()
        .into_inner()
        .model
        .unwrap();
    assert!(!inspected.runtime_available);
    let live_inspected = model
        .inspect_model(InspectModelRequest {
            identity: Some(rpc_identity("model-a", "1", "rev-a")),
            observe_live_health: true,
            deadline_epoch_ms: deadline(),
        })
        .await
        .unwrap()
        .into_inner()
        .model
        .unwrap();
    assert_eq!(live_inspected.health, ModelHealth::Unavailable as i32);
    assert_eq!(
        live_inspected.error.unwrap().code,
        "model_runtime_unavailable"
    );
    assert_eq!(
        model
            .preload_model(PreloadModelRequest {
                operation: Some(operation("preload-a")),
                deadline_epoch_ms: deadline(),
                identity: Some(rpc_identity("model-a", "1", "rev-a"))
            })
            .await
            .unwrap()
            .into_inner()
            .error
            .unwrap()
            .code,
        "model_runtime_unavailable"
    );
    let stage_b = import_root.join("stage-b");
    std::fs::create_dir_all(&stage_b).unwrap();
    write_package(&stage_b, "model-b", "1", "rev-b");
    let manifest_b = format!(
        "{:x}",
        Sha256::digest(std::fs::read(stage_b.join("manifest.yaml")).unwrap())
    );
    assert_eq!(
        model
            .import_staged_model(ImportStagedModelRequest {
                operation: Some(operation("import-b")),
                deadline_epoch_ms: deadline(),
                stage_id: "stage-b".into(),
                expected_identity: Some(rpc_identity("model-b", "1", "rev-b")),
                expected_manifest_sha256: manifest_b,
            })
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    for (operation_id, response) in [
        (
            "activate-no-provider",
            model
                .activate_model(ActivateModelRequest {
                    operation: Some(operation("activate-no-provider")),
                    deadline_epoch_ms: deadline(),
                    identity: Some(rpc_identity("model-a", "1", "rev-a")),
                })
                .await
                .unwrap()
                .into_inner(),
        ),
        (
            "rollback-no-provider",
            model
                .rollback_model(RollbackModelRequest {
                    operation: Some(operation("rollback-no-provider")),
                    deadline_epoch_ms: deadline(),
                    from_identity: Some(rpc_identity("model-a", "1", "rev-a")),
                    to_identity: Some(rpc_identity("model-b", "1", "rev-b")),
                })
                .await
                .unwrap()
                .into_inner(),
        ),
    ] {
        assert_eq!(response.operation_id, operation_id);
        assert_eq!(response.error.unwrap().code, "model_runtime_unavailable");
    }
    for (page_size, expected_error) in [(0, false), (200, false), (201, true)] {
        let page = model
            .list_models(ListModelsRequest {
                page_size,
                page_token: String::new(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(page.error.is_some(), expected_error);
        if !expected_error {
            assert_eq!(page.models.len(), 2);
        }
    }
    let first_page = model
        .list_models(ListModelsRequest {
            page_size: 1,
            page_token: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first_page.models.len(), 1);
    assert!(!first_page.next_page_token.is_empty());
    let second_page = model
        .list_models(ListModelsRequest {
            page_size: 1,
            page_token: first_page.next_page_token,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(second_page.models.len(), 1);
    assert!(second_page.next_page_token.is_empty());
    assert_eq!(
        model
            .list_models(ListModelsRequest {
                page_size: 1,
                page_token: "not base64!".into()
            })
            .await
            .unwrap()
            .into_inner()
            .error
            .unwrap()
            .code,
        "model_page_token_invalid"
    );

    let traversal = model
        .import_staged_model(ImportStagedModelRequest {
            operation: Some(operation("traversal")),
            deadline_epoch_ms: deadline(),
            stage_id: "../escape".into(),
            expected_identity: Some(rpc_identity("x", "1", "r")),
            expected_manifest_sha256: "0".repeat(64),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(traversal.error.unwrap().code, "model_stage_invalid");
    let target = root.path().join("target-stage");
    std::fs::create_dir_all(&target).unwrap();
    let link = import_root.join("link-stage");
    symlink(&target, &link).unwrap();
    let linked = model
        .import_staged_model(ImportStagedModelRequest {
            operation: Some(operation("symlink")),
            deadline_epoch_ms: deadline(),
            stage_id: "link-stage".into(),
            expected_identity: Some(rpc_identity("x", "1", "r")),
            expected_manifest_sha256: "0".repeat(64),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(linked.error.unwrap().code, "model_stage_insecure");

    cancel.cancel();
    server.await.unwrap().unwrap();
    assert!(!socket.exists());
    tasks.close_and_wait().await.unwrap();
}

#[tokio::test]
async fn real_sqlite_pending_import_reconciles_after_crash_and_terminal_replay_is_stable() {
    use crate::model::{
        ClaimOperation, OperationClaimRequest, OperationReceiptLimits, OperationReceiptState,
    };
    let root = TestRoot::new("receipt-crash");
    let import_root = root.path().join("import");
    let stage = import_root.join("stage-a");
    std::fs::create_dir_all(&stage).unwrap();
    write_package(&stage, "model-a", "1", "rev-a");
    let package = verify_package(&stage, &policy()).unwrap();
    let manifest_hash = package.manifest_sha256.clone();
    let db = root.path().join("model.db");
    let models = root.path().join("models");
    let repository = ModelRepository::open(&db, &models).await.unwrap();
    let request_deadline = deadline();
    let command_hash = {
        let request = ImportStagedModelRequest {
            operation: Some(operation("crash-import")),
            deadline_epoch_ms: request_deadline,
            stage_id: "stage-a".into(),
            expected_identity: Some(rpc_identity("model-a", "1", "rev-a")),
            expected_manifest_sha256: manifest_hash.clone(),
        };
        let manager = ModelManager::open(
            repository.clone(),
            Vec::new(),
            ModelManagerConfig::default(),
        )
        .await
        .unwrap();
        let tasks = task_manager(&root, manager.clone(), "hash").await;
        let rpc = AvaiModelManagementRpc::new(
            repository.clone(),
            manager,
            tasks.clone(),
            management_config(&root),
        )
        .unwrap();
        let response = rpc
            .import_staged_model(Request::new(request.clone()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.error, None);
        tasks.close_and_wait().await.unwrap();
        // Use a second operation to leave a real durable PENDING receipt, then commit its side effect.
        let pending_hash = "pending-crash-hash";
        assert!(matches!(
            repository
                .claim_operation(
                    OperationClaimRequest {
                        operation_id: "pending-crash",
                        idempotency_key: "key-pending-crash",
                        operation_kind: "IMPORT",
                        request_hash: pending_hash,
                        deadline_epoch_ms: request_deadline,
                        now_epoch_ms: deadline() - 60_000,
                    },
                    OperationReceiptLimits {
                        retention_ms: 24 * 60 * 60 * 1_000,
                        capacity: 16,
                    },
                )
                .await
                .unwrap(),
            ClaimOperation::New(_)
        ));
        pending_hash.to_string()
    };
    assert_eq!(command_hash, "pending-crash-hash");
    repository.install(&package, deadline()).await.unwrap();
    repository.close().await;

    let reopened = ModelRepository::open(&db, &models).await.unwrap();
    let receipt = reopened
        .claim_operation(
            OperationClaimRequest {
                operation_id: "pending-crash",
                idempotency_key: "key-pending-crash",
                operation_kind: "IMPORT",
                request_hash: "pending-crash-hash",
                deadline_epoch_ms: request_deadline,
                now_epoch_ms: deadline() - 60_000,
            },
            OperationReceiptLimits {
                retention_ms: 24 * 60 * 60 * 1_000,
                capacity: 16,
            },
        )
        .await
        .unwrap();
    let pending = match receipt {
        ClaimOperation::Existing(receipt) => receipt,
        ClaimOperation::New(_) => panic!("pending receipt was lost"),
    };
    assert_eq!(pending.state, OperationReceiptState::Pending);
    assert_eq!(
        reopened
            .get(&identity("model-a", "1", "rev-a"))
            .await
            .unwrap()
            .unwrap()
            .manifest_sha256,
        manifest_hash
    );
    reopened
        .finish_operation(
            "pending-crash",
            OperationReceiptState::Succeeded,
            None,
            deadline(),
        )
        .await
        .unwrap();
    let replay = reopened
        .claim_operation(
            OperationClaimRequest {
                operation_id: "pending-crash",
                idempotency_key: "key-pending-crash",
                operation_kind: "IMPORT",
                request_hash: "pending-crash-hash",
                deadline_epoch_ms: request_deadline,
                now_epoch_ms: deadline() - 60_000,
            },
            OperationReceiptLimits {
                retention_ms: 24 * 60 * 60 * 1_000,
                capacity: 16,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(replay, ClaimOperation::Existing(receipt) if receipt.state == OperationReceiptState::Succeeded)
    );
}

#[tokio::test]
async fn exact_multi_capability_rollback_is_atomic_and_partial_drift_fails_closed() {
    let root = TestRoot::new("exact-rollback");
    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    install_test_model(
        &repository,
        &root,
        "model-a",
        "1",
        "rev-a",
        &["vehicle.detect", "vision.object.detect"],
    )
    .await;
    install_test_model(
        &repository,
        &root,
        "model-b",
        "2",
        "rev-b",
        &["vehicle.detect", "vision.object.detect"],
    )
    .await;
    let providers: Vec<Arc<dyn RuntimeProvider>> = vec![Arc::new(FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior::default(),
    ))];
    let manager = ModelManager::open(repository, providers, ModelManagerConfig::default())
        .await
        .unwrap();
    let a = identity("model-a", "1", "rev-a");
    let b = identity("model-b", "2", "rev-b");
    manager.preload(&a, 1).await.unwrap();
    manager.activate(&a, 2).await.unwrap();
    manager.preload(&b, 3).await.unwrap();
    manager.activate(&b, 4).await.unwrap();
    manager.rollback_exact(&b, &a, 5).await.unwrap();
    let a_observation = manager.observation(&a).await;
    let b_observation = manager.observation(&b).await;
    assert_eq!(
        a_observation.active_capabilities,
        vec!["vehicle.detect", "vision.object.detect"]
    );
    assert_eq!(
        b_observation.previous_capabilities,
        vec!["vehicle.detect", "vision.object.detect"]
    );
    manager.rollback("vehicle.detect", 6).await.unwrap();
    let before = manager.observation(&a).await;
    assert_eq!(
        manager.rollback_exact(&a, &b, 7).await.unwrap_err().code,
        "model_rollback_conflict"
    );
    assert_eq!(manager.observation(&a).await, before);
}

#[tokio::test]
async fn caller_loss_keeps_bounded_owner_command_alive_and_replay_observes_completion() {
    let root = TestRoot::new("caller-loss");
    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    install_test_model(
        &repository,
        &root,
        "model-a",
        "1",
        "rev-a",
        &["vehicle.detect"],
    )
    .await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    fake.pause_health_checks();
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let tasks = task_manager(&root, manager.clone(), "caller-loss").await;
    let rpc = AvaiModelManagementRpc::new(
        repository,
        manager.clone(),
        tasks.clone(),
        management_config(&root),
    )
    .unwrap();
    let terminal_request = UnloadModelRequest {
        operation: Some(operation("terminal-before-busy")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    let terminal = rpc
        .unload_model(Request::new(terminal_request.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(terminal.error, None);
    assert!(!terminal.replayed);
    let request = PreloadModelRequest {
        operation: Some(operation("lost-preload")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    let detached_rpc = rpc.clone();
    let detached_request = request.clone();
    let caller = base::tokio::spawn(async move {
        detached_rpc
            .preload_model(Request::new(detached_request))
            .await
    });
    for _ in 0..200 {
        if fake.started_health_checks() != 0 {
            break;
        }
        base::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_ne!(fake.started_health_checks(), 0);
    caller.abort();
    let terminal_replay = rpc
        .unload_model(Request::new(terminal_request))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(terminal_replay.error, None);
    assert!(terminal_replay.replayed);
    let busy = rpc
        .activate_model(Request::new(ActivateModelRequest {
            operation: Some(operation("while-busy")),
            deadline_epoch_ms: deadline(),
            identity: Some(rpc_identity("model-a", "1", "rev-a")),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(busy.error.unwrap().code, "model_operation_busy");
    fake.release_health_checks();
    let replay = loop {
        let response = rpc
            .preload_model(Request::new(request.clone()))
            .await
            .unwrap()
            .into_inner();
        if response.replayed {
            break response;
        }
        base::tokio::task::yield_now().await;
    };
    assert_eq!(replay.error, None);
    assert!(
        manager
            .observation(&identity("model-a", "1", "rev-a"))
            .await
            .loaded
    );
    tasks.close_and_wait().await.unwrap();
}

#[tokio::test]
async fn delayed_terminal_activate_and_rollback_never_execute_twice_and_unload_guard_is_durable() {
    let root = TestRoot::new("terminal-replay");
    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    for (model, version, revision, capabilities) in [
        (
            "model-a",
            "1",
            "rev-a",
            vec!["vehicle.detect", "vision.object.detect"],
        ),
        (
            "model-b",
            "2",
            "rev-b",
            vec!["vehicle.detect", "vision.object.detect"],
        ),
        ("model-c", "1", "rev-c", vec!["independent.detect"]),
    ] {
        install_test_model(&repository, &root, model, version, revision, &capabilities).await;
    }
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior::default(),
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let tasks = task_manager(&root, manager.clone(), "terminal-replay").await;
    let rpc = AvaiModelManagementRpc::new(
        repository,
        manager.clone(),
        tasks.clone(),
        management_config(&root),
    )
    .unwrap();
    for (operation_id, model, version, revision) in [
        ("preload-a", "model-a", "1", "rev-a"),
        ("preload-b", "model-b", "2", "rev-b"),
        ("preload-c", "model-c", "1", "rev-c"),
    ] {
        assert_eq!(
            rpc.preload_model(Request::new(PreloadModelRequest {
                operation: Some(operation(operation_id)),
                deadline_epoch_ms: deadline(),
                identity: Some(rpc_identity(model, version, revision)),
            }))
            .await
            .unwrap()
            .into_inner()
            .error,
            None
        );
    }
    let activate_a = ActivateModelRequest {
        operation: Some(operation("activate-a")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    assert_eq!(
        rpc.activate_model(Request::new(activate_a.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    assert_eq!(
        rpc.activate_model(Request::new(ActivateModelRequest {
            operation: Some(operation("activate-b")),
            deadline_epoch_ms: deadline(),
            identity: Some(rpc_identity("model-b", "2", "rev-b"))
        }))
        .await
        .unwrap()
        .into_inner()
        .error,
        None
    );
    let replay_activate = rpc
        .activate_model(Request::new(activate_a))
        .await
        .unwrap()
        .into_inner();
    assert!(replay_activate.replayed);
    assert_eq!(
        manager
            .observation(&identity("model-b", "2", "rev-b"))
            .await
            .active_capabilities,
        vec!["vehicle.detect", "vision.object.detect"]
    );

    let rollback = RollbackModelRequest {
        operation: Some(operation("rollback-b-a")),
        deadline_epoch_ms: deadline(),
        from_identity: Some(rpc_identity("model-b", "2", "rev-b")),
        to_identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    assert_eq!(
        rpc.rollback_model(Request::new(rollback.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    assert_eq!(
        rpc.activate_model(Request::new(ActivateModelRequest {
            operation: Some(operation("activate-b-again")),
            deadline_epoch_ms: deadline(),
            identity: Some(rpc_identity("model-b", "2", "rev-b"))
        }))
        .await
        .unwrap()
        .into_inner()
        .error,
        None
    );
    let replay_rollback = rpc
        .rollback_model(Request::new(rollback))
        .await
        .unwrap()
        .into_inner();
    assert!(replay_rollback.replayed);
    assert_eq!(
        manager
            .observation(&identity("model-b", "2", "rev-b"))
            .await
            .active_capabilities,
        vec!["vehicle.detect", "vision.object.detect"]
    );

    tasks
        .insert_nonterminal_task_for_test("durable-pending")
        .await
        .unwrap();
    let unload = rpc
        .unload_model(Request::new(UnloadModelRequest {
            operation: Some(operation("unload-c")),
            deadline_epoch_ms: deadline(),
            identity: Some(rpc_identity("model-c", "1", "rev-c")),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unload.error.unwrap().code, "model_in_use");
    assert!(
        manager
            .observation(&identity("model-c", "1", "rev-c"))
            .await
            .loaded
    );
    tasks.close_and_wait().await.unwrap();
}

#[tokio::test]
async fn receipt_capacity_conflict_and_retention_cleanup_are_mechanically_bounded() {
    use crate::model::{
        ClaimOperation, OperationClaimRequest, OperationReceiptLimits, OperationReceiptState,
    };
    let root = TestRoot::new("receipt-bounds");
    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    let retention = 24 * 60 * 60 * 1_000;
    let now = deadline() - 60_000;
    let limits = || OperationReceiptLimits {
        retention_ms: retention,
        capacity: 1,
    };
    let claim = |operation_id: &'static str,
                 key: &'static str,
                 hash: &'static str,
                 at: i64,
                 deadline_epoch_ms: i64| OperationClaimRequest {
        operation_id,
        idempotency_key: key,
        operation_kind: "PRELOAD",
        request_hash: hash,
        deadline_epoch_ms,
        now_epoch_ms: at,
    };
    assert!(matches!(
        repository
            .claim_operation(claim("op-1", "key-1", "hash-1", now, now + 1_000), limits())
            .await
            .unwrap(),
        ClaimOperation::New(_)
    ));
    assert_eq!(
        repository
            .claim_operation(claim("op-2", "key-2", "hash-2", now, now + 1_000), limits())
            .await
            .unwrap_err()
            .code,
        "model_operation_capacity_exceeded"
    );
    assert_eq!(
        repository
            .claim_operation(
                claim("op-1", "key-1", "changed", now, now + 1_000),
                limits()
            )
            .await
            .unwrap_err()
            .code,
        "model_operation_conflict"
    );
    repository
        .finish_operation("op-1", OperationReceiptState::Succeeded, None, now + 2_000)
        .await
        .unwrap();
    let after_retention = now + retention + 3_001;
    assert!(matches!(
        repository
            .claim_operation(
                claim(
                    "op-2",
                    "key-2",
                    "hash-2",
                    after_retention,
                    after_retention + 1_000
                ),
                limits()
            )
            .await
            .unwrap(),
        ClaimOperation::New(_)
    ));
}

#[tokio::test]
async fn real_sqlite_pending_receipts_reconcile_every_runtime_lifecycle_side_effect() {
    let root = TestRoot::new("runtime-crash-windows");
    let repository =
        ModelRepository::open(&root.path().join("model.db"), &root.path().join("models"))
            .await
            .unwrap();
    for (model, version, revision, capabilities) in [
        ("model-a", "1", "rev-a", vec!["vehicle.detect"]),
        ("model-b", "2", "rev-b", vec!["vehicle.detect"]),
        ("model-c", "1", "rev-c", vec!["independent.detect"]),
    ] {
        install_test_model(&repository, &root, model, version, revision, &capabilities).await;
    }
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let tasks = task_manager(&root, manager.clone(), "runtime-crash-windows").await;
    let rpc = AvaiModelManagementRpc::new(
        repository.clone(),
        manager.clone(),
        tasks.clone(),
        management_config(&root),
    )
    .unwrap();

    let preload_a = PreloadModelRequest {
        operation: Some(operation("crash-preload-a")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    assert_eq!(
        rpc.preload_model(Request::new(preload_a.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    let health_count = fake.started_health_checks();
    repository
        .reset_operation_pending_for_test("crash-preload-a")
        .await
        .unwrap();
    let replay = rpc
        .preload_model(Request::new(preload_a))
        .await
        .unwrap()
        .into_inner();
    assert!(replay.replayed);
    assert_eq!(fake.started_health_checks(), health_count);

    let activate_a = ActivateModelRequest {
        operation: Some(operation("crash-activate-a")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    assert_eq!(
        rpc.activate_model(Request::new(activate_a.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    let generation_a = manager
        .observation(&identity("model-a", "1", "rev-a"))
        .await
        .generation;
    repository
        .reset_operation_pending_for_test("crash-activate-a")
        .await
        .unwrap();
    assert!(
        rpc.activate_model(Request::new(activate_a))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
    assert_eq!(
        manager
            .observation(&identity("model-a", "1", "rev-a"))
            .await
            .generation,
        generation_a
    );

    let preload_b = PreloadModelRequest {
        operation: Some(operation("preload-b-for-rollback")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-b", "2", "rev-b")),
    };
    assert_eq!(
        rpc.preload_model(Request::new(preload_b))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    assert_eq!(
        rpc.activate_model(Request::new(ActivateModelRequest {
            operation: Some(operation("activate-b-for-rollback")),
            deadline_epoch_ms: deadline(),
            identity: Some(rpc_identity("model-b", "2", "rev-b"))
        }))
        .await
        .unwrap()
        .into_inner()
        .error,
        None
    );
    let rollback = RollbackModelRequest {
        operation: Some(operation("crash-rollback")),
        deadline_epoch_ms: deadline(),
        from_identity: Some(rpc_identity("model-b", "2", "rev-b")),
        to_identity: Some(rpc_identity("model-a", "1", "rev-a")),
    };
    assert_eq!(
        rpc.rollback_model(Request::new(rollback.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    let rollback_generation = manager
        .observation(&identity("model-a", "1", "rev-a"))
        .await
        .generation;
    repository
        .reset_operation_pending_for_test("crash-rollback")
        .await
        .unwrap();
    assert!(
        rpc.rollback_model(Request::new(rollback))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
    assert_eq!(
        manager
            .observation(&identity("model-a", "1", "rev-a"))
            .await
            .generation,
        rollback_generation
    );

    let preload_c = PreloadModelRequest {
        operation: Some(operation("preload-c-for-unload")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-c", "1", "rev-c")),
    };
    assert_eq!(
        rpc.preload_model(Request::new(preload_c))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    let unload_c = UnloadModelRequest {
        operation: Some(operation("crash-unload-c")),
        deadline_epoch_ms: deadline(),
        identity: Some(rpc_identity("model-c", "1", "rev-c")),
    };
    assert_eq!(
        rpc.unload_model(Request::new(unload_c.clone()))
            .await
            .unwrap()
            .into_inner()
            .error,
        None
    );
    assert!(
        !manager
            .observation(&identity("model-c", "1", "rev-c"))
            .await
            .loaded
    );
    repository
        .reset_operation_pending_for_test("crash-unload-c")
        .await
        .unwrap();
    assert!(
        rpc.unload_model(Request::new(unload_c))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
    assert!(
        !manager
            .observation(&identity("model-c", "1", "rev-c"))
            .await
            .loaded
    );
    tasks.close_and_wait().await.unwrap();
}
