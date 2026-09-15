use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use avai::{
    model::{
        ModelManager, ModelManagerConfig, ModelPackageManifest, ModelRepository, OnnxCpuConfig,
        OnnxCpuProvider, PackagePolicy, RuntimeCallContext, RuntimeInput, RuntimeProvider,
        model_package_signing_payload, verify_package,
    },
    model_management::{AvaiModelManagementRpc, ModelManagementConfig},
    source::SourcePolicy,
    task::{TaskManager, TaskManagerConfig},
};
use base::{base64::Engine, sha2::Digest, utils::rt::GlobalRuntime};
use ed25519_dalek::{Signer, SigningKey};
use gmv_protocol::{
    avai::model_management::v1::{
        InspectModelRequest, ModelIdentity as RpcModelIdentity, PreloadModelRequest,
        avai_model_management_server::AvaiModelManagement,
    },
    avai::v1::{
        AiTaskState, CreateTaskRequest, ImageMetadata, OwnedImageRef, QueryTaskRequest, SourceSpec,
        source_spec,
    },
    common::v1::{AccessGrant, DataEndpoint, NodeIdentity, NodeKind, OperationRef, ResourceRef},
};
use tonic::Request;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/onnx_cpu");

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("avai-native-onnx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn policy() -> PackagePolicy {
    PackagePolicy {
        available_runtimes: HashSet::from(["onnx-cpu".to_string()]),
        allowed_result_schemas: HashSet::from([("gmv.tensor.outputs".to_string(), 1)]),
        approved_spdx: HashSet::from(["Apache-2.0".to_string()]),
        trusted_signing_keys: HashMap::from([(
            "test-key".to_string(),
            SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        )]),
        max_memory_mb: 1024,
        max_vram_mb: 0,
        ..PackagePolicy::default()
    }
}

fn context(timeout: Duration) -> RuntimeCallContext {
    RuntimeCallContext {
        deadline: Instant::now() + timeout,
        cancellation: base::tokio_util::sync::CancellationToken::new(),
    }
}

fn write_package(root: &Path, revision: &str, model_file: &str, size: u32, fail_oracle: bool) {
    let source = Path::new(FIXTURE);
    let files = [
        ("model/model.onnx", model_file),
        ("tests/input.png", "input.png"),
        ("tests/expected.json", "expected.json"),
        ("schema/result.schema.json", "result.schema.json"),
    ];
    for (target, fixture) in files {
        let target = root.join(target);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(source.join(fixture), target).unwrap();
    }
    if fail_oracle {
        std::fs::write(
            root.join("tests/expected.json"),
            br#"{"outputs":[{"name":"output","dtype":"f32","shape":[1,3,1,1],"data":[0,0,0]}]}"#,
        )
        .unwrap();
    } else if model_file == "termination-stress.onnx" {
        std::fs::write(
            root.join("tests/expected.json"),
            br#"{"outputs":[{"name":"output","dtype":"f32","shape":[1,3,1,1],"data":[-0.10615816,0.10720716,-0.10736427]}]}"#,
        )
        .unwrap();
    }
    let listed = [
        "model/model.onnx",
        "tests/input.png",
        "tests/expected.json",
        "schema/result.schema.json",
    ];
    let file_yaml = listed
        .iter()
        .map(|relative| {
            let bytes = std::fs::read(root.join(relative)).unwrap();
            format!(
                "  - path: {relative}\n    sha256: {:x}\n    size: {}",
                base::sha2::Sha256::digest(&bytes),
                bytes.len()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let unsigned = format!(
        "api_version: gmv.ai/v1\nkind: ModelPlugin\nmetadata:\n  model_id: add-rgb\n  version: '1'\n  revision: {revision}\ncapabilities:\n  - tensor.test\nresult_schema:\n  name: gmv.tensor.outputs\n  version: 1\n  path: schema/result.schema.json\nvariants:\n  - runtime: onnx-cpu\n    runtime_contract_version: 1\n    architecture: {}\n    accelerator: cpu\n    artifact: model/model.onnx\nexecution:\n  version: 1\n  input:\n    kind: encoded_image_tensor_v1\n    accepted_media_types: [image/png]\n    max_bytes: 1024\n    max_width: 1\n    max_height: 1\n    tensor:\n      name: input\n      dtype: f32\n      layout: nchw\n      shape: [1, 3, {size}, {size}]\n    preprocess:\n      resize: exact\n      interpolation: bilinear\n      color: rgb\n      scale: 1.0\n      mean: [0.0, 0.0, 0.0]\n      std: [1.0, 1.0, 1.0]\n  outputs:\n    - name: output\n      dtype: f32\n      shape: [1, 3, {size}, {size}]\n  postprocess:\n    kind: tensor_json_v1\nresources:\n  memory_mb: 256\n  vram_mb: 0\n  max_batch: 1\nlicense:\n  spdx: Apache-2.0\n  commercial_use: true\n  redistribution: allowed\n  license_ref: ''\nself_test:\n  - input: tests/input.png\n    expected: tests/expected.json\n    oracle:\n      kind: json_numeric_v1\n      abs_tolerance: 0.0\n      rel_tolerance: 0.0\nfiles:\n{file_yaml}\nsigning:\n  key_id: test-key\n  signature: ''\n",
        std::env::consts::ARCH
    )
    .replace(
        "abs_tolerance: 0.0\n      rel_tolerance: 0.0",
        "abs_tolerance: 0.00001\n      rel_tolerance: 0.00001",
    );
    let manifest: ModelPackageManifest = base::serde_yaml::from_str(&unsigned).unwrap();
    let signature =
        SigningKey::from_bytes(&[7; 32]).sign(&model_package_signing_payload(&manifest).unwrap());
    std::fs::write(
        root.join("manifest.yaml"),
        unsigned.replace(
            "signature: ''",
            &format!(
                "signature: {}",
                base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            ),
        ),
    )
    .unwrap();
}

fn runtime_input() -> RuntimeInput {
    let encoded = std::fs::read(Path::new(FIXTURE).join("input.png")).unwrap();
    RuntimeInput {
        encoded: encoded.into(),
        media_type: "image/png".to_string(),
        width: 1,
        height: 1,
    }
}

fn deadline_after(duration: Duration) -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .saturating_add(duration)
        .as_millis() as i64
}

fn rpc_identity(revision: &str) -> RpcModelIdentity {
    RpcModelIdentity {
        model_id: "add-rgb".to_string(),
        version: "1".to_string(),
        revision: revision.to_string(),
    }
}

async fn wait_for_native_job(provider: &OnnxCpuProvider, previous_admitted: usize) {
    for _ in 0..200 {
        let snapshot = provider.lifetime_snapshot();
        if snapshot.admitted_jobs > previous_admitted && snapshot.active_jobs > 0 {
            return;
        }
        base::tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("native work was not observed in the fixed executor");
}

async fn wait_for_task(
    tasks: &TaskManager,
    task_id: &str,
) -> gmv_protocol::avai::v1::QueryTaskResponse {
    for _ in 0..500 {
        let response = tasks
            .query_task(QueryTaskRequest {
                task_id: task_id.to_string(),
            })
            .await;
        match AiTaskState::try_from(response.state) {
            Ok(AiTaskState::Succeeded) => return response,
            Ok(AiTaskState::Failed) => panic!("native task {task_id} failed: {:?}", response.error),
            _ => base::tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    panic!("native task {task_id} did not terminate");
}

async fn create_owned_task(
    tasks: &TaskManager,
    node: &NodeIdentity,
    task_id: &str,
    object_id: &str,
    input_bytes: &[u8],
    now_epoch_ms: i64,
) {
    tasks
        .create_task(
            CreateTaskRequest {
                operation: Some(OperationRef {
                    operation_id: format!("operation-{task_id}"),
                    idempotency_key: format!("idempotency-{task_id}"),
                }),
                task_id: task_id.to_string(),
                capability: "tensor.test".to_string(),
                expected_avai: Some(node.clone()),
                source: Some(SourceSpec {
                    source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                        owner: Some(node.clone()),
                        resource: Some(ResourceRef {
                            resource_id: object_id.to_string(),
                            resource_type: "avai_image".to_string(),
                        }),
                        metadata: Some(ImageMetadata {
                            content_type: "image/png".to_string(),
                            size_bytes: input_bytes.len() as u64,
                            sha256: format!("{:x}", base::sha2::Sha256::digest(input_bytes)),
                            width: 1,
                            height: 1,
                        }),
                        access: Some(AccessGrant {
                            grant_id: format!("grant-{task_id}"),
                            expected_consumer: Some(node.clone()),
                            purpose: "tensor.test".to_string(),
                            expires_at_epoch_ms: i64::MAX,
                            endpoints: vec![DataEndpoint {
                                name: "image".to_string(),
                                uri: format!("gmv-object://{object_id}"),
                                ..Default::default()
                            }],
                            proof: vec![1],
                        }),
                    })),
                }),
                ..Default::default()
            },
            now_epoch_ms,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_onnx_cpu_acceptance_and_cooperative_termination() {
    let library = std::env::var("AVAI_TEST_ORT_LIBRARY")
        .expect("AVAI_TEST_ORT_LIBRARY must point to the prepared ORT 1.28.0 CPU library");
    let missing = OnnxCpuProvider::initialize_for_test(
        "/definitely/missing/libonnxruntime.so",
        Default::default(),
    )
    .err()
    .unwrap();
    assert_eq!(missing.code, "model_runtime_unavailable");
    let provider = OnnxCpuProvider::initialize_for_test(
        &library,
        OnnxCpuConfig {
            worker_count: 2,
            queue_capacity: 1,
            intra_threads: 1,
            inter_threads: 1,
            max_result_bytes: 1024 * 1024,
            shutdown_timeout: Duration::from_secs(5),
            execution_limits: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(provider.descriptor().runtime, "onnx-cpu");
    assert_eq!(provider.descriptor().version, "1.28.0");

    let root = TestRoot::new("acceptance");
    let package_root = root.0.join("good");
    std::fs::create_dir_all(&package_root).unwrap();
    write_package(&package_root, "good", "model.onnx", 1, false);
    let package = verify_package(&package_root, &policy()).unwrap();
    let repository = ModelRepository::open(&root.0.join("models.db"), &root.0.join("models"))
        .await
        .unwrap();
    let installed = repository.install(&package, 1).await.unwrap();
    assert_eq!(
        installed.selected_variant.as_ref().unwrap().artifact_sha256,
        format!(
            "{:x}",
            base::sha2::Sha256::digest(
                std::fs::read(package_root.join("model/model.onnx")).unwrap()
            )
        )
    );
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    manager.preload(&installed.identity, 2).await.unwrap();
    manager.activate(&installed.identity, 3).await.unwrap();
    let active = manager.capture("tensor.test").await.unwrap();
    let result = active
        .infer(runtime_input(), context(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(result.actual_model.model_id, "add-rgb");
    assert_eq!(result.actual_model.revision, "good");
    assert_eq!(result.actual_model.runtime, "onnx-cpu");
    let output: base::serde_json::Value = base::serde_json::from_slice(&result.output).unwrap();
    assert_eq!(
        output["outputs"][0]["data"],
        base::serde_json::json!([11.0, 22.0, 33.0])
    );

    let mut invalid = runtime_input();
    invalid.width = 2;
    assert_eq!(
        active
            .infer(invalid, context(Duration::from_secs(5)))
            .await
            .unwrap_err()
            .code,
        "model_input_contract_mismatch"
    );

    drop(active);
    drop(manager);
    let restarted = ModelManager::open(
        repository.clone(),
        vec![Arc::new(provider.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        restarted.capture("tensor.test").await.unwrap().identity(),
        &installed.identity
    );

    let object_root = root.0.join("objects");
    std::fs::create_dir_all(&object_root).unwrap();
    let object_id = "native-owned-image";
    let input_bytes = std::fs::read(Path::new(FIXTURE).join("input.png")).unwrap();
    std::fs::write(object_root.join(object_id), &input_bytes).unwrap();
    let node = NodeIdentity {
        node_id: "avai-native-test".to_string(),
        instance_id: "instance-native-test".to_string(),
        kind: NodeKind::Avai as i32,
    };
    let task_runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::Custom(
        "avai-native-onnx-task".to_string(),
    ))
    .unwrap();
    let tasks = TaskManager::open_with_model_manager(
        node.clone(),
        vec!["tensor.test".to_string()],
        TaskManagerConfig {
            database_path: root.0.join("models.db"),
            queue_size: 2,
            worker_count: 1,
            source_policy: SourcePolicy {
                object_root,
                ..SourcePolicy::default()
            },
            max_result_bytes: 1024 * 1024,
        },
        Some(restarted.clone()),
        &task_runtime,
    )
    .await
    .unwrap();
    tasks
        .create_task(
            CreateTaskRequest {
                operation: Some(OperationRef {
                    operation_id: "native-task-operation".to_string(),
                    idempotency_key: "native-task-idempotency".to_string(),
                }),
                task_id: "native-owned-image-task".to_string(),
                capability: "tensor.test".to_string(),
                expected_avai: Some(node.clone()),
                source: Some(SourceSpec {
                    source: Some(source_spec::Source::OwnedImage(OwnedImageRef {
                        owner: Some(node.clone()),
                        resource: Some(ResourceRef {
                            resource_id: object_id.to_string(),
                            resource_type: "avai_image".to_string(),
                        }),
                        metadata: Some(ImageMetadata {
                            content_type: "image/png".to_string(),
                            size_bytes: input_bytes.len() as u64,
                            sha256: format!("{:x}", base::sha2::Sha256::digest(&input_bytes)),
                            width: 1,
                            height: 1,
                        }),
                        access: Some(AccessGrant {
                            grant_id: "native-task-grant".to_string(),
                            expected_consumer: Some(node.clone()),
                            purpose: "tensor.test".to_string(),
                            expires_at_epoch_ms: i64::MAX,
                            endpoints: vec![DataEndpoint {
                                name: "image".to_string(),
                                uri: format!("gmv-object://{object_id}"),
                                ..Default::default()
                            }],
                            proof: vec![1],
                        }),
                    })),
                }),
                ..Default::default()
            },
            10,
        )
        .await;
    let mut task_result = None;
    for _ in 0..200 {
        let response = tasks
            .query_task(QueryTaskRequest {
                task_id: "native-owned-image-task".to_string(),
            })
            .await;
        if AiTaskState::try_from(response.state) == Ok(AiTaskState::Succeeded) {
            task_result = Some(response);
            break;
        }
        assert_ne!(
            AiTaskState::try_from(response.state),
            Ok(AiTaskState::Failed)
        );
        base::tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        task_result
            .expect("non-Stream native task did not succeed")
            .typed_result
            .unwrap()
            .actual_model
            .unwrap()
            .runtime,
        "onnx-cpu"
    );
    let failed_root = root.0.join("failed");
    std::fs::create_dir_all(&failed_root).unwrap();
    write_package(&failed_root, "failed", "model.onnx", 1, true);
    let failed = repository
        .install(&verify_package(&failed_root, &policy()).unwrap(), 4)
        .await
        .unwrap();
    assert_eq!(
        restarted
            .preload(&failed.identity, 5)
            .await
            .unwrap_err()
            .code,
        "model_self_test_failed"
    );
    assert_eq!(
        restarted.capture("tensor.test").await.unwrap().identity(),
        &installed.identity
    );

    let deadline_root = root.0.join("deadline");
    std::fs::create_dir_all(&deadline_root).unwrap();
    write_package(
        &deadline_root,
        "deadline",
        "termination-stress.onnx",
        1,
        false,
    );
    let _deadline_model = repository
        .install(&verify_package(&deadline_root, &policy()).unwrap(), 6)
        .await
        .unwrap();
    let management = AvaiModelManagementRpc::new(
        repository.clone(),
        restarted.clone(),
        tasks.clone(),
        ModelManagementConfig {
            trusted_import_root: root.0.join("import"),
            package_policy: policy(),
            receipt_capacity: 16,
            receipt_retention_ms: 24 * 60 * 60 * 1_000,
            mutation_concurrency: 1,
        },
    )
    .unwrap();
    let before_deadline = provider.lifetime_snapshot();
    let deadline_response = management
        .preload_model(Request::new(PreloadModelRequest {
            operation: Some(OperationRef {
                operation_id: "native-deadline-preload".to_string(),
                idempotency_key: "native-deadline-preload-key".to_string(),
            }),
            deadline_epoch_ms: deadline_after(Duration::from_millis(20)),
            identity: Some(rpc_identity("deadline")),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        deadline_response.error.unwrap().code,
        "model_runtime_deadline_exceeded"
    );
    let after_deadline = provider.lifetime_snapshot();
    assert_eq!(after_deadline.active_jobs, 0);
    assert_eq!(after_deadline.admitted_jobs, after_deadline.completed_jobs);
    assert!(after_deadline.admitted_jobs > before_deadline.admitted_jobs);

    let stress_root = root.0.join("stress");
    std::fs::create_dir_all(&stress_root).unwrap();
    write_package(&stress_root, "stress", "termination-stress.onnx", 1, false);
    let stress = repository
        .install(&verify_package(&stress_root, &policy()).unwrap(), 7)
        .await
        .unwrap();
    let stress_instance = provider
        .preload(&stress, context(Duration::from_secs(10)))
        .await
        .unwrap();
    let first_cancel = base::tokio_util::sync::CancellationToken::new();
    let second_cancel = base::tokio_util::sync::CancellationToken::new();
    let third_cancel = base::tokio_util::sync::CancellationToken::new();
    let started = Instant::now();
    let first_instance = stress_instance.clone();
    let first_token = first_cancel.clone();
    let first = base::tokio::spawn(async move {
        first_instance
            .infer(
                runtime_input(),
                RuntimeCallContext {
                    deadline: Instant::now() + Duration::from_secs(10),
                    cancellation: first_token,
                },
            )
            .await
    });
    base::tokio::time::sleep(Duration::from_millis(20)).await;
    let second_instance = stress_instance.clone();
    let second_token = second_cancel.clone();
    let second = base::tokio::spawn(async move {
        second_instance
            .infer(
                runtime_input(),
                RuntimeCallContext {
                    deadline: Instant::now() + Duration::from_secs(10),
                    cancellation: second_token,
                },
            )
            .await
    });
    base::tokio::time::sleep(Duration::from_millis(20)).await;
    let third_instance = stress_instance.clone();
    let third_token = third_cancel.clone();
    let third = base::tokio::spawn(async move {
        third_instance
            .infer(
                runtime_input(),
                RuntimeCallContext {
                    deadline: Instant::now() + Duration::from_secs(10),
                    cancellation: third_token,
                },
            )
            .await
    });
    base::tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        stress_instance
            .infer(runtime_input(), context(Duration::from_secs(1)))
            .await
            .unwrap_err()
            .code,
        "model_runtime_busy"
    );
    first_cancel.cancel();
    second_cancel.cancel();
    third_cancel.cancel();
    let first_error = base::tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("Architecture Stop Condition: ORT termination exceeded five seconds")
        .unwrap()
        .unwrap_err();
    let second_error = base::tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("Architecture Stop Condition: queued cancellation exceeded five seconds")
        .unwrap()
        .unwrap_err();
    let third_error = base::tokio::time::timeout(Duration::from_secs(5), third)
        .await
        .expect("Architecture Stop Condition: queued cancellation exceeded five seconds")
        .unwrap()
        .unwrap_err();
    assert_eq!(first_error.code, "model_runtime_cancelled");
    assert_eq!(second_error.code, "model_runtime_cancelled");
    assert_eq!(third_error.code, "model_runtime_cancelled");
    assert!(started.elapsed() < Duration::from_secs(6));
    restarted.preload(&stress.identity, 8).await.unwrap();
    restarted.activate(&stress.identity, 9).await.unwrap();
    let health_cancel = base::tokio_util::sync::CancellationToken::new();
    let health_manager = restarted.clone();
    let health_identity = stress.identity.clone();
    let health_token = health_cancel.clone();
    let before_health = provider.lifetime_snapshot();
    let health = base::tokio::spawn(async move {
        health_manager
            .health_with_context(
                &health_identity,
                RuntimeCallContext {
                    deadline: Instant::now() + Duration::from_secs(5),
                    cancellation: health_token,
                },
            )
            .await
    });
    wait_for_native_job(&provider, before_health.admitted_jobs).await;
    health_cancel.cancel();
    assert_eq!(
        base::tokio::time::timeout(Duration::from_secs(5), health)
            .await
            .expect("Architecture Stop Condition: live health cancellation did not drain")
            .unwrap()
            .unwrap_err()
            .code,
        "model_runtime_cancelled"
    );
    let after_health = provider.lifetime_snapshot();
    assert_eq!(after_health.active_jobs, 0);
    assert_eq!(after_health.admitted_jobs, after_health.completed_jobs);

    let before_inspect = provider.lifetime_snapshot();
    let inspected = management
        .inspect_model(Request::new(InspectModelRequest {
            identity: Some(rpc_identity("stress")),
            observe_live_health: true,
            deadline_epoch_ms: deadline_after(Duration::from_millis(20)),
        }))
        .await
        .unwrap()
        .into_inner()
        .model
        .unwrap();
    assert_eq!(inspected.error.unwrap().code, "model_deadline_exceeded");
    let after_inspect = provider.lifetime_snapshot();
    assert_eq!(after_inspect.active_jobs, 0);
    assert_eq!(after_inspect.admitted_jobs, after_inspect.completed_jobs);
    assert!(after_inspect.admitted_jobs > before_inspect.admitted_jobs);

    let b_root = root.0.join("revision-b");
    std::fs::create_dir_all(&b_root).unwrap();
    write_package(&b_root, "b", "model.onnx", 1, false);
    let revision_b = repository
        .install(&verify_package(&b_root, &policy()).unwrap(), 10)
        .await
        .unwrap();
    restarted.preload(&revision_b.identity, 11).await.unwrap();

    let before_a_task = provider.lifetime_snapshot();
    create_owned_task(&tasks, &node, "native-task-a", object_id, &input_bytes, 12).await;
    wait_for_native_job(&provider, before_a_task.admitted_jobs).await;
    restarted.activate(&revision_b.identity, 13).await.unwrap();
    create_owned_task(&tasks, &node, "native-task-b", object_id, &input_bytes, 14).await;
    assert_eq!(
        restarted
            .retire_previous("tensor.test", 15)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );
    let task_a = wait_for_task(&tasks, "native-task-a").await;
    let task_b = wait_for_task(&tasks, "native-task-b").await;
    assert_eq!(
        task_a.typed_result.unwrap().actual_model.unwrap().revision,
        "stress"
    );
    assert_eq!(
        task_b.typed_result.unwrap().actual_model.unwrap().revision,
        "b"
    );
    assert_eq!(
        restarted.retire_previous("tensor.test", 16).await.unwrap(),
        Some(stress.identity.clone())
    );
    restarted.unload(&stress.identity, 17).await.unwrap();

    let small_result_provider = OnnxCpuProvider::initialize_for_test(
        &library,
        OnnxCpuConfig {
            worker_count: 1,
            queue_capacity: 1,
            intra_threads: 1,
            inter_threads: 1,
            max_result_bytes: 8,
            shutdown_timeout: Duration::from_secs(5),
            execution_limits: Default::default(),
        },
    )
    .unwrap();
    let small_result_instance = small_result_provider
        .preload(&installed, context(Duration::from_secs(5)))
        .await
        .unwrap();
    let small_result_error = small_result_instance
        .infer(runtime_input(), context(Duration::from_secs(5)))
        .await
        .unwrap_err();
    assert_eq!(small_result_error.code, "result_too_large");
    drop(small_result_instance);
    small_result_provider.close_and_wait().await.unwrap();

    restarted.preload(&stress.identity, 18).await.unwrap();
    restarted.activate(&stress.identity, 19).await.unwrap();
    let before_shutdown = provider.lifetime_snapshot();
    create_owned_task(
        &tasks,
        &node,
        "native-shutdown-task",
        object_id,
        &input_bytes,
        20,
    )
    .await;
    wait_for_native_job(&provider, before_shutdown.admitted_jobs).await;
    drop(management);
    task_runtime.cancel.cancel();
    base::tokio::time::timeout(Duration::from_secs(5), provider.close_and_wait())
        .await
        .expect("Architecture Stop Condition: native executor shutdown did not drain")
        .unwrap();
    base::tokio::time::timeout(Duration::from_secs(5), tasks.close_and_wait())
        .await
        .expect("TaskManager did not join after bounded native drain")
        .unwrap();
    let shutdown = provider.lifetime_snapshot();
    assert!(!shutdown.accepting);
    assert_eq!(shutdown.active_jobs, 0);
    assert_eq!(shutdown.admitted_jobs, shutdown.completed_jobs);
    assert_eq!(shutdown.workers_remaining, 0);

    drop(tasks);
    drop(stress_instance);
    drop(restarted);
    repository.close().await;
    let released = provider.lifetime_snapshot();
    assert_eq!(released.instances_created, released.instances_dropped);
}
