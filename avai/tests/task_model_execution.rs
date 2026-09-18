use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use avai::{
    model::{
        FakeRuntimeBehavior, FakeRuntimeProvider, ModelIdentity, ModelManager, ModelManagerConfig,
        ModelPackageManifest, ModelRepository, PackagePolicy, RuntimeProvider,
        model_package_signing_payload, verify_package,
    },
    observability::Observability,
    source::SourcePolicy,
    task::{TaskManager, TaskManagerConfig},
};
use base::tokio_util::sync::CancellationToken;
use base::{
    base64::Engine,
    sha2::{Digest, Sha256},
    utils::rt::GlobalRuntime,
};
use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{Row, SqlitePool},
};
use ed25519_dalek::{Signer, SigningKey};
use gmv_protocol::{
    avai::v1::{
        AiTaskState, CancelTaskRequest, CreateTaskRequest, ImageMetadata, ModelRef, OwnedImageRef,
        QueryTaskRequest, SourceSpec, source_spec,
    },
    common::v1::{AccessGrant, DataEndpoint, NodeIdentity, NodeKind, OperationRef, ResourceRef},
};
use prost::Message;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);
const CAPABILITY: &str = "vehicle.detect";
const OTHER_CAPABILITY: &str = "vision.other";
const BUILTIN_CAPABILITY: &str = "image.metadata.inspect";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "avai-task-model-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn database_path(&self) -> PathBuf {
        self.0.join("avai.db")
    }

    fn model_root(&self) -> PathBuf {
        self.0.join("models")
    }

    fn object_root(&self) -> PathBuf {
        self.0.join("objects")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity(model_id: &str, version: &str, revision: &str) -> ModelIdentity {
    ModelIdentity {
        model_id: model_id.to_string(),
        version: version.to_string(),
        revision: revision.to_string(),
    }
}

fn node_identity() -> NodeIdentity {
    NodeIdentity {
        node_id: "avai-test".to_string(),
        instance_id: "instance-test".to_string(),
        kind: NodeKind::Avai as i32,
    }
}

fn policy() -> PackagePolicy {
    PackagePolicy {
        available_runtimes: HashSet::from(["fake".to_string()]),
        allowed_result_schemas: HashSet::from([("gmv.vision.observation".to_string(), 1)]),
        approved_spdx: HashSet::from(["Apache-2.0".to_string()]),
        trusted_signing_keys: HashMap::from([(
            "test-key".to_string(),
            SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        )]),
        max_memory_mb: 128,
        max_vram_mb: 0,
        ..PackagePolicy::default()
    }
}

fn write_package(
    root: &Path,
    model_id: &str,
    version: &str,
    revision: &str,
    capabilities: &[&str],
) {
    let files = [
        ("model/model.bin", b"fake-model".as_slice()),
        (
            "schema/result.schema.json",
            br#"{"type":"object"}"#.as_slice(),
        ),
        ("tests/input.bin", b"input".as_slice()),
        ("tests/expected.json", br#"{"ok":true}"#.as_slice()),
    ];
    for (relative, bytes) in files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let file_yaml = files
        .iter()
        .map(|(path, bytes)| {
            format!(
                "  - path: {path}\n    sha256: {:x}\n    size: {}",
                Sha256::digest(bytes),
                bytes.len()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let capabilities_yaml = capabilities
        .iter()
        .map(|capability| format!("  - {capability}"))
        .collect::<Vec<_>>()
        .join("\n");
    let unsigned = format!(
        "api_version: gmv.ai/v1\nkind: ModelPlugin\nmetadata:\n  model_id: {model_id}\n  version: {version}\n  revision: {revision}\ncapabilities:\n{capabilities_yaml}\nresult_schema:\n  name: gmv.vision.observation\n  version: 1\n  path: schema/result.schema.json\nvariants:\n  - runtime: fake\n    runtime_contract_version: 1\n    architecture: {}\n    accelerator: cpu\n    artifact: model/model.bin\nresources:\n  memory_mb: 64\n  vram_mb: 0\n  max_batch: 4\nlicense:\n  spdx: Apache-2.0\n  commercial_use: true\n  redistribution: allowed\n  license_ref: \"\"\nself_test:\n  - input: tests/input.bin\n    expected: tests/expected.json\nfiles:\n{file_yaml}\nsigning:\n  key_id: test-key\n  signature: \"\"\n",
        std::env::consts::ARCH
    );
    let manifest: ModelPackageManifest = base::serde_yaml::from_str(&unsigned).unwrap();
    let signature =
        SigningKey::from_bytes(&[7; 32]).sign(&model_package_signing_payload(&manifest).unwrap());
    std::fs::write(
        root.join("manifest.yaml"),
        unsigned.replace(
            "signature: \"\"",
            &format!(
                "signature: {}",
                base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            ),
        ),
    )
    .unwrap();
}

async fn install_model(
    repository: &ModelRepository,
    root: &TestRoot,
    model_id: &str,
    version: &str,
    revision: &str,
    capabilities: &[&str],
) {
    let source = root.path().join(format!("source-{revision}"));
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, model_id, version, revision, capabilities);
    let package = verify_package(&source, &policy()).unwrap();
    repository.install(&package, 1).await.unwrap();
}

async fn model_repository(root: &TestRoot) -> ModelRepository {
    ModelRepository::open(&root.database_path(), &root.model_root())
        .await
        .unwrap()
}

fn runtime(name: &str) -> GlobalRuntime {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(format!(
        "avai-task-model-{name}-{id}"
    )))
    .unwrap()
}

fn task_config(root: &TestRoot, worker_count: usize, max_result_bytes: usize) -> TaskManagerConfig {
    std::fs::create_dir_all(root.object_root()).unwrap();
    TaskManagerConfig {
        database_path: root.database_path(),
        queue_size: 16,
        worker_count,
        source_policy: SourcePolicy {
            object_root: root.object_root(),
            ..SourcePolicy::default()
        },
        max_result_bytes,
    }
}

fn write_task_object(root: &TestRoot, task_id: &str) -> (String, ImageMetadata) {
    let object_id = format!("object-{task_id}");
    let bytes = base::base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap();
    std::fs::create_dir_all(root.object_root()).unwrap();
    std::fs::write(root.object_root().join(&object_id), &bytes).unwrap();
    let metadata = ImageMetadata {
        content_type: "image/png".to_string(),
        size_bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        width: 1,
        height: 1,
    };
    (object_id, metadata)
}

fn task_request(
    root: &TestRoot,
    task_id: &str,
    capability: &str,
    requested_model: Option<ModelRef>,
    deadline_epoch_ms: i64,
) -> CreateTaskRequest {
    let (object_id, metadata) = write_task_object(root, task_id);
    CreateTaskRequest {
        operation: Some(OperationRef {
            operation_id: format!("operation-{task_id}"),
            idempotency_key: format!("idempotency-{task_id}"),
        }),
        task_id: task_id.to_string(),
        capability: capability.to_string(),
        expected_avai: Some(node_identity()),
        source: Some(SourceSpec {
            source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                owner: Some(node_identity()),
                resource: Some(ResourceRef {
                    resource_id: object_id.clone(),
                    resource_type: "avai_image".to_string(),
                }),
                metadata: Some(metadata),
                access: Some(AccessGrant {
                    grant_id: format!("grant-{task_id}"),
                    expected_consumer: Some(node_identity()),
                    purpose: capability.to_string(),
                    expires_at_epoch_ms: now_epoch_ms() + 60_000,
                    endpoints: vec![DataEndpoint {
                        name: "image".to_string(),
                        uri: format!("gmv-object://{object_id}"),
                        ..Default::default()
                    }],
                    proof: vec![1],
                }),
            })),
        }),
        requested_model,
        deadline_epoch_ms,
        ..Default::default()
    }
}

fn model_ref(model_id: &str, version: &str, revision: &str, runtime: &str) -> ModelRef {
    ModelRef {
        model_id: model_id.to_string(),
        version: version.to_string(),
        revision: revision.to_string(),
        runtime: runtime.to_string(),
    }
}

async fn wait_terminal(
    manager: &TaskManager,
    task_id: &str,
) -> gmv_protocol::avai::v1::QueryTaskResponse {
    for _ in 0..200 {
        let response = manager
            .query_task(QueryTaskRequest {
                task_id: task_id.to_string(),
            })
            .await;
        if matches!(
            AiTaskState::try_from(response.state),
            Ok(AiTaskState::Succeeded | AiTaskState::Failed | AiTaskState::Cancelled)
        ) {
            return response;
        }
        base::tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task did not reach a terminal state: {task_id}");
}

async fn wait_started(fake: &FakeRuntimeProvider, count: usize) {
    for _ in 0..200 {
        if fake.started_inferences() >= count {
            return;
        }
        base::tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("inference did not start");
}

async fn wait_retired(
    manager: &ModelManager,
    capability: &str,
    now_epoch_ms: i64,
) -> ModelIdentity {
    for _ in 0..100 {
        match manager.retire_previous(capability, now_epoch_ms).await {
            Ok(Some(identity)) => return identity,
            Err(error) if error.code == "model_in_use" => {
                base::tokio::time::sleep(Duration::from_millis(5)).await;
            }
            other => panic!("unexpected retire result: {other:?}"),
        }
    }
    panic!("captured model reference was not released");
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
async fn dispatch_capture_survives_activation_switch_and_releases_old_revision() {
    let root = TestRoot::new("dispatch-switch");
    let repository = model_repository(&root).await;
    install_model(&repository, &root, "model-a", "1", "rev-a", &[CAPABILITY]).await;
    install_model(&repository, &root, "model-b", "2", "rev-b", &[CAPABILITY]).await;
    let fake = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            block_inference: true,
            inference_output: Some(br#"{"ok":true}"#.to_vec()),
            ..Default::default()
        },
    );
    let telemetry = Arc::new(Observability::new());
    let manager = ModelManager::open_with_observability(
        repository.clone(),
        vec![Arc::new(fake.clone()) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
        CancellationToken::new(),
        telemetry.clone(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 10).await.unwrap();
    manager.preload(&model_b, 11).await.unwrap();
    manager.activate(&model_a, 12).await.unwrap();
    let task_runtime = runtime("dispatch-switch");
    let tasks = TaskManager::open_with_model_manager_and_observability(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(manager.clone()),
        &task_runtime,
        telemetry.clone(),
    )
    .await
    .unwrap();

    tasks
        .create_task(
            task_request(&root, "task-a", CAPABILITY, None, 0),
            now_epoch_ms(),
        )
        .await;
    wait_started(&fake, 1).await;
    tasks
        .create_task(
            task_request(&root, "task-b", CAPABILITY, None, 0),
            now_epoch_ms(),
        )
        .await;
    manager.activate(&model_b, 13).await.unwrap();
    assert_eq!(
        manager
            .retire_previous(CAPABILITY, 14)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );

    fake.release_inferences();
    let first = wait_terminal(&tasks, "task-a").await;
    let first_result = first.typed_result.unwrap();
    let output = first_result.output.as_ref().unwrap();
    assert_eq!(output.schema, "gmv.vision.observation");
    assert_eq!(output.version, 1);
    let first_model = first_result.actual_model.unwrap();
    assert_eq!(first_model, model_ref("model-a", "1", "rev-a", "fake"));
    wait_started(&fake, 2).await;
    fake.release_inferences();
    let second = wait_terminal(&tasks, "task-b").await;
    let second_model = second.typed_result.unwrap().actual_model.unwrap();
    assert_eq!(second_model, model_ref("model-b", "2", "rev-b", "fake"));
    let task_metrics = telemetry.snapshot();
    assert_eq!(task_metrics["tasks_actual_model_slot_00_succeeded"], "1");
    assert!(task_metrics["tasks_actual_model_slot_00_identity"].contains("model-a"));
    assert_eq!(task_metrics["tasks_actual_model_slot_01_succeeded"], "1");
    assert!(task_metrics["tasks_actual_model_slot_01_identity"].contains("model-b"));
    tasks
        .cancel_task(CancelTaskRequest {
            task_id: "task-a".to_string(),
            ..Default::default()
        })
        .await;
    let after_duplicate = telemetry.snapshot();
    assert_eq!(after_duplicate["tasks_actual_model_slot_00_succeeded"], "1");
    assert_eq!(after_duplicate["tasks_actual_model_slot_00_cancelled"], "0");

    assert_eq!(wait_retired(&manager, CAPABILITY, 15).await, model_a);
    manager.unload(&model_a, 16).await.unwrap();
    assert_eq!(fake.dropped_instances("model-a"), 1);
    tasks.close_and_wait().await.unwrap();
    repository.close().await;
}

#[tokio::test]
async fn terminal_before_binding_is_counted_once_without_actual_model() {
    let root = TestRoot::new("terminal-without-binding");
    let task_runtime = runtime("terminal-without-binding");
    let telemetry = Arc::new(Observability::new());
    let tasks = TaskManager::open_with_model_manager_and_observability(
        node_identity(),
        vec![BUILTIN_CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        None,
        &task_runtime,
        telemetry.clone(),
    )
    .await
    .unwrap();
    tasks
        .create_task(
            task_request(
                &root,
                "task-unbound",
                BUILTIN_CAPABILITY,
                Some(model_ref("missing", "1", "rev-a", "fake")),
                0,
            ),
            now_epoch_ms(),
        )
        .await;
    let terminal = wait_terminal(&tasks, "task-unbound").await;
    assert_eq!(terminal.state, AiTaskState::Failed as i32);
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot["tasks_without_actual_model_total"], "1");
    assert!(!snapshot.keys().any(|key| key.contains("slot_00")));
    tasks
        .cancel_task(CancelTaskRequest {
            task_id: "task-unbound".to_string(),
            ..Default::default()
        })
        .await;
    assert_eq!(
        telemetry.snapshot()["tasks_without_actual_model_total"],
        "1"
    );
    tasks.close_and_wait().await.unwrap();
}

#[tokio::test]
async fn requested_model_policy_is_exact_ready_only_and_never_implicitly_preloads() {
    let root = TestRoot::new("requested-policy");
    let repository = model_repository(&root).await;
    for (id, version, revision, capabilities) in [
        ("model-a", "1", "rev-a", vec![CAPABILITY]),
        ("model-b", "2", "rev-b", vec![CAPABILITY]),
        ("model-c", "3", "rev-c", vec![CAPABILITY]),
        ("model-d", "4", "rev-d", vec![CAPABILITY]),
        ("model-e", "5", "rev-e", vec![OTHER_CAPABILITY]),
    ] {
        install_model(&repository, &root, id, version, revision, &capabilities).await;
    }
    let fake = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            fail_preload_model: Some("model-d".to_string()),
            inference_output: Some(br#"{"ok":true}"#.to_vec()),
            ..Default::default()
        },
    );
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 10).await.unwrap();
    manager.preload(&model_b, 11).await.unwrap();
    manager.activate(&model_a, 12).await.unwrap();
    assert_eq!(
        manager
            .preload(&identity("model-d", "4", "rev-d"), 13)
            .await
            .unwrap_err()
            .code,
        "model_preload_failed"
    );
    let task_runtime = runtime("requested-policy");
    let tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(manager.clone()),
        &task_runtime,
    )
    .await
    .unwrap();

    for (task_id, requested, expected_state, expected_code) in [
        (
            "requested-active",
            model_ref("model-a", "1", "rev-a", "fake"),
            AiTaskState::Succeeded,
            "",
        ),
        (
            "requested-ready",
            model_ref("model-b", "2", "rev-b", ""),
            AiTaskState::Succeeded,
            "",
        ),
        (
            "requested-installed",
            model_ref("model-c", "3", "rev-c", "fake"),
            AiTaskState::Failed,
            "model_not_ready",
        ),
        (
            "requested-failed",
            model_ref("model-d", "4", "rev-d", "fake"),
            AiTaskState::Failed,
            "model_failed",
        ),
        (
            "requested-missing",
            model_ref("missing", "1", "rev-x", "fake"),
            AiTaskState::Failed,
            "model_not_found",
        ),
        (
            "requested-capability",
            model_ref("model-e", "5", "rev-e", "fake"),
            AiTaskState::Failed,
            "model_capability_incompatible",
        ),
        (
            "requested-runtime",
            model_ref("model-a", "1", "rev-a", "other"),
            AiTaskState::Failed,
            "model_runtime_incompatible",
        ),
        (
            "requested-no-revision",
            model_ref("model-a", "1", "", "fake"),
            AiTaskState::Failed,
            "model_revision_required",
        ),
    ] {
        tasks
            .create_task(
                task_request(&root, task_id, CAPABILITY, Some(requested), 0),
                now_epoch_ms(),
            )
            .await;
        let terminal = wait_terminal(&tasks, task_id).await;
        assert_eq!(terminal.state, expected_state as i32, "{task_id}");
        if expected_code.is_empty() {
            assert!(terminal.error.is_none(), "{task_id}");
        } else {
            assert_eq!(terminal.error.unwrap().code, expected_code, "{task_id}");
        }
    }
    assert_eq!(
        manager.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    tasks.close_and_wait().await.unwrap();
    repository.close().await;
}

#[tokio::test]
async fn recovered_binding_reacquires_exact_unloaded_revision_and_never_falls_back() {
    let root = TestRoot::new("recovery");
    let repository = model_repository(&root).await;
    install_model(&repository, &root, "model-a", "1", "rev-a", &[CAPABILITY]).await;
    install_model(&repository, &root, "model-b", "2", "rev-b", &[CAPABILITY]).await;
    let blocking = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            block_inference: true,
            inference_output: Some(br#"{"ok":true}"#.to_vec()),
            ..Default::default()
        },
    );
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(blocking.clone()) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 10).await.unwrap();
    manager.preload(&model_b, 11).await.unwrap();
    manager.activate(&model_a, 12).await.unwrap();
    let first_runtime = runtime("recovery-first");
    let first_tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(manager.clone()),
        &first_runtime,
    )
    .await
    .unwrap();
    first_tasks
        .create_task(
            task_request(&root, "bound-a", CAPABILITY, None, 0),
            now_epoch_ms(),
        )
        .await;
    wait_started(&blocking, 1).await;
    first_tasks.close_and_wait().await.unwrap();
    manager.activate(&model_b, 13).await.unwrap();
    assert_eq!(wait_retired(&manager, CAPABILITY, 14).await, model_a);
    manager.unload(&model_a, 15).await.unwrap();
    drop(manager);

    let replay = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            inference_output: Some(br#"{"ok":true}"#.to_vec()),
            ..Default::default()
        },
    );
    let recovery_telemetry = Arc::new(Observability::new());
    let restarted = ModelManager::open_with_observability(
        repository.clone(),
        vec![Arc::new(replay) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
        CancellationToken::new(),
        recovery_telemetry.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_b
    );
    let second_runtime = runtime("recovery-second");
    let second_tasks = TaskManager::open_with_model_manager_and_observability(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(restarted),
        &second_runtime,
        recovery_telemetry.clone(),
    )
    .await
    .unwrap();
    let replayed = wait_terminal(&second_tasks, "bound-a").await;
    assert_eq!(
        replayed.typed_result.unwrap().actual_model.unwrap(),
        model_ref("model-a", "1", "rev-a", "fake")
    );
    let recovery_metrics = recovery_telemetry.snapshot();
    assert_eq!(recovery_metrics["ready_models"], "2");
    assert_eq!(recovery_metrics["preload_seconds_count"], "2");

    let pool = open_sqlite(&root.database_path()).await;
    let missing = task_request(&root, "bound-missing", CAPABILITY, None, 0);
    let unbound = task_request(&root, "unbound-pending", CAPABILITY, None, 0);
    let binding = base::serde_json::to_vec(&base::serde_json::json!({
        "version": 1,
        "kind": "managed",
        "capability": CAPABILITY,
        "model_id": "missing",
        "model_version": "1",
        "revision": "rev-missing",
        "runtime": "fake",
        "result_schema_name": "gmv.vision.observation",
        "result_schema_version": 1
    }))
    .unwrap();
    insert_raw_task(&pool, &missing, AiTaskState::Running, Some(&binding)).await;
    insert_raw_task(&pool, &unbound, AiTaskState::Pending, None).await;
    pool.close().await;
    second_tasks.close_and_wait().await.unwrap();

    let final_manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior {
                inference_output: Some(br#"{"ok":true}"#.to_vec()),
                ..Default::default()
            },
        )) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let final_runtime = runtime("recovery-missing");
    let final_tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(final_manager),
        &final_runtime,
    )
    .await
    .unwrap();
    let unavailable = wait_terminal(&final_tasks, "bound-missing").await;
    assert_eq!(unavailable.error.unwrap().code, "bound_model_unavailable");
    let unbound = wait_terminal(&final_tasks, "unbound-pending").await;
    assert_eq!(
        unbound.typed_result.unwrap().actual_model.unwrap(),
        model_ref("model-b", "2", "rev-b", "fake")
    );
    final_tasks.close_and_wait().await.unwrap();
    repository.close().await;
}

#[tokio::test]
async fn cancellation_and_absolute_deadline_release_captured_executions() {
    let root = TestRoot::new("cancel-deadline");
    let repository = model_repository(&root).await;
    for (id, version, revision) in [
        ("model-a", "1", "rev-a"),
        ("model-b", "2", "rev-b"),
        ("model-c", "3", "rev-c"),
    ] {
        install_model(&repository, &root, id, version, revision, &[CAPABILITY]).await;
    }
    let fake = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            block_inference: true,
            ignore_inference_cancellation: true,
            inference_output: Some(br#"{"ok":true}"#.to_vec()),
            ..Default::default()
        },
    );
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone()) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    let model_c = identity("model-c", "3", "rev-c");
    for (model, time) in [(&model_a, 10), (&model_b, 11), (&model_c, 12)] {
        manager.preload(model, time).await.unwrap();
    }
    manager.activate(&model_a, 13).await.unwrap();
    let task_runtime = runtime("cancel-deadline");
    let tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(manager.clone()),
        &task_runtime,
    )
    .await
    .unwrap();

    tasks
        .create_task(
            task_request(&root, "cancelled", CAPABILITY, None, 0),
            now_epoch_ms(),
        )
        .await;
    wait_started(&fake, 1).await;
    manager.activate(&model_b, 14).await.unwrap();
    let cancelled = tasks
        .cancel_task(CancelTaskRequest {
            task_id: "cancelled".to_string(),
            ..Default::default()
        })
        .await;
    assert_eq!(cancelled.state, AiTaskState::Cancelled as i32);
    assert_eq!(
        manager
            .retire_previous(CAPABILITY, 15)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );
    fake.release_inferences();
    assert_eq!(wait_retired(&manager, CAPABILITY, 15).await, model_a);

    tasks
        .create_task(
            task_request(&root, "deadline", CAPABILITY, None, now_epoch_ms() + 80),
            now_epoch_ms(),
        )
        .await;
    wait_started(&fake, 2).await;
    manager.activate(&model_c, 16).await.unwrap();
    let expired = wait_terminal(&tasks, "deadline").await;
    assert_eq!(expired.error.unwrap().code, "task_expired");
    assert_eq!(
        manager
            .retire_previous(CAPABILITY, 17)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );
    fake.release_inferences();
    assert_eq!(wait_retired(&manager, CAPABILITY, 17).await, model_b);
    tasks.close_and_wait().await.unwrap();
    repository.close().await;
}

#[tokio::test]
async fn managed_results_are_bounded_valid_json_and_builtin_conflicts_fail_closed() {
    for (name, output, limit, expected_code) in [
        (
            "oversized",
            br#"{"value":"too-large"}"#.to_vec(),
            4,
            "result_too_large",
        ),
        ("invalid", b"not-json".to_vec(), 1024, "invalid_result_json"),
    ] {
        let root = TestRoot::new(name);
        let repository = model_repository(&root).await;
        install_model(&repository, &root, "model-a", "1", "rev-a", &[CAPABILITY]).await;
        let manager = ModelManager::open(
            repository.clone(),
            vec![Arc::new(FakeRuntimeProvider::new(
                "fake",
                FakeRuntimeBehavior {
                    inference_output: Some(output),
                    ..Default::default()
                },
            )) as Arc<dyn RuntimeProvider>],
            ModelManagerConfig::default(),
        )
        .await
        .unwrap();
        let model_a = identity("model-a", "1", "rev-a");
        manager.preload(&model_a, 10).await.unwrap();
        manager.activate(&model_a, 11).await.unwrap();
        let task_runtime = runtime(name);
        let tasks = TaskManager::open_with_model_manager(
            node_identity(),
            vec![CAPABILITY.to_string()],
            task_config(&root, 1, limit),
            Some(manager),
            &task_runtime,
        )
        .await
        .unwrap();
        tasks
            .create_task(
                task_request(&root, name, CAPABILITY, None, 0),
                now_epoch_ms(),
            )
            .await;
        assert_eq!(
            wait_terminal(&tasks, name).await.error.unwrap().code,
            expected_code
        );
        tasks.close_and_wait().await.unwrap();
        repository.close().await;
    }

    let root = TestRoot::new("selection-conflict");
    let repository = model_repository(&root).await;
    install_model(
        &repository,
        &root,
        "managed-builtin-capability",
        "1",
        "rev-a",
        &[BUILTIN_CAPABILITY],
    )
    .await;
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior {
                inference_output: Some(br#"{"ok":true}"#.to_vec()),
                ..Default::default()
            },
        )) as Arc<dyn RuntimeProvider>],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let managed = identity("managed-builtin-capability", "1", "rev-a");
    manager.preload(&managed, 10).await.unwrap();
    manager.activate(&managed, 11).await.unwrap();
    let task_runtime = runtime("selection-conflict");
    let tasks = TaskManager::open_with_model_manager(
        node_identity(),
        vec![BUILTIN_CAPABILITY.to_string()],
        task_config(&root, 1, 1024),
        Some(manager),
        &task_runtime,
    )
    .await
    .unwrap();
    tasks
        .create_task(
            task_request(&root, "conflict", BUILTIN_CAPABILITY, None, 0),
            now_epoch_ms(),
        )
        .await;
    assert_eq!(
        wait_terminal(&tasks, "conflict").await.error.unwrap().code,
        "model_selection_conflict"
    );
    tasks.close_and_wait().await.unwrap();
    repository.close().await;
}

#[tokio::test]
async fn exact_legacy_task_schema_upgrades_idempotently_and_preserves_rows() {
    let root = TestRoot::new("legacy-schema");
    let pool = open_sqlite(&root.database_path()).await;
    base_db::sqlx::query(
        "CREATE TABLE avai_task (\
         task_id TEXT PRIMARY KEY NOT NULL,\
         idempotency_key TEXT NOT NULL UNIQUE,\
         request_hash TEXT NOT NULL,\
         request BLOB NOT NULL,\
         capability TEXT NOT NULL,\
         route_id TEXT NOT NULL,\
         state INTEGER NOT NULL,\
         result BLOB NULL,\
         error_code TEXT NULL,\
         error_message TEXT NULL,\
         created_at_ms INTEGER NOT NULL,\
         updated_at_ms INTEGER NOT NULL,\
         terminal_at_ms INTEGER NULL\
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    base_db::sqlx::query(
        "INSERT INTO avai_task(task_id,idempotency_key,request_hash,request,capability,route_id,state,created_at_ms,updated_at_ms,terminal_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?)",
    )
    .bind("legacy-task")
    .bind("legacy-idempotency")
    .bind("legacy-hash")
    .bind(Vec::<u8>::new())
    .bind(BUILTIN_CAPABILITY)
    .bind("")
    .bind(AiTaskState::Succeeded as i32)
    .bind(1_i64)
    .bind(2_i64)
    .bind(2_i64)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    for pass in 0..2 {
        let task_runtime = runtime(&format!("legacy-schema-{pass}"));
        let tasks = TaskManager::open(
            node_identity(),
            vec![BUILTIN_CAPABILITY.to_string()],
            task_config(&root, 1, 1024),
            &task_runtime,
        )
        .await
        .unwrap();
        tasks.close_and_wait().await.unwrap();
    }
    let pool = open_sqlite(&root.database_path()).await;
    let columns = base_db::sqlx::query("PRAGMA table_info(avai_task)")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        columns
            .iter()
            .filter(|row| row.try_get::<String, _>("name").unwrap() == "execution_binding")
            .count(),
        1
    );
    let row =
        base_db::sqlx::query("SELECT state, execution_binding FROM avai_task WHERE task_id=?")
            .bind("legacy-task")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row.try_get::<i32, _>("state").unwrap(),
        AiTaskState::Succeeded as i32
    );
    assert!(
        row.try_get::<Option<Vec<u8>>, _>("execution_binding")
            .unwrap()
            .is_none()
    );
    pool.close().await;
}

async fn open_sqlite(path: &Path) -> SqlitePool {
    base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(path),
        DatabasePoolConfig {
            max_size: 4,
            min_idle: Some(1),
            ..Default::default()
        },
    )
    .unwrap()
}

async fn insert_raw_task(
    pool: &SqlitePool,
    request: &CreateTaskRequest,
    state: AiTaskState,
    binding: Option<&[u8]>,
) {
    base_db::sqlx::query(
        "INSERT INTO avai_task(task_id,idempotency_key,request_hash,request,capability,route_id,state,execution_binding,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&request.task_id)
    .bind(&request.operation.as_ref().unwrap().idempotency_key)
    .bind(format!("hash-{}", request.task_id))
    .bind(request.encode_to_vec())
    .bind(&request.capability)
    .bind(&request.route_id)
    .bind(state as i32)
    .bind(binding)
    .bind(now_epoch_ms())
    .bind(now_epoch_ms())
    .execute(pool)
    .await
    .unwrap();
}
