use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use avai::model::{
    FakeRuntimeBehavior, FakeRuntimeProvider, ModelIdentity, ModelManager, ModelManagerConfig,
    ModelPackageManifest, ModelRepository, ModelState, PackagePolicy, RuntimeProvider,
    model_package_signing_payload, verify_package,
};
use base::{
    base64::Engine,
    sha2::{Digest, Sha256},
};
use ed25519_dalek::{Signer, SigningKey};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);
const CAPABILITY: &str = "vehicle.detect";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "avai-model-runtime-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_package(root: &Path, model_id: &str, version: &str, revision: &str) {
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
    let unsigned_manifest = format!(
        "api_version: gmv.ai/v1\nkind: ModelPlugin\nmetadata:\n  model_id: {model_id}\n  version: {version}\n  revision: {revision}\ncapabilities:\n  - {CAPABILITY}\nresult_schema:\n  name: gmv.vision.observation\n  version: 1\n  path: schema/result.schema.json\nvariants:\n  - runtime: fake\n    architecture: {}\n    accelerator: cpu\n    artifact: model/model.bin\nresources:\n  memory_mb: 64\n  vram_mb: 0\n  max_batch: 4\nlicense:\n  spdx: Apache-2.0\n  commercial_use: true\n  redistribution: allowed\n  license_ref: \"\"\nself_test:\n  - input: tests/input.bin\n    expected: tests/expected.json\nfiles:\n{file_yaml}\nsigning:\n  key_id: test-key\n  signature: \"\"\n",
        std::env::consts::ARCH
    );
    let manifest = sign_manifest(&unsigned_manifest);
    std::fs::write(root.join("manifest.yaml"), manifest).unwrap();
}

fn sign_manifest(unsigned_manifest: &str) -> String {
    let parsed: ModelPackageManifest = base::serde_yaml::from_str(unsigned_manifest).unwrap();
    let signature =
        SigningKey::from_bytes(&[7; 32]).sign(&model_package_signing_payload(&parsed).unwrap());
    unsigned_manifest.replace(
        "signature: \"\"",
        &format!(
            "signature: {}",
            base::base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
        ),
    )
}

fn resign_manifest(manifest: &str) -> String {
    let signature_line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("signature:"))
        .unwrap();
    sign_manifest(&manifest.replace(signature_line, "  signature: \"\""))
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

fn identity(model_id: &str, version: &str, revision: &str) -> ModelIdentity {
    ModelIdentity {
        model_id: model_id.to_string(),
        version: version.to_string(),
        revision: revision.to_string(),
    }
}

async fn repository_with_models(root: &TestRoot) -> ModelRepository {
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    for (model_id, version, revision) in [("model-a", "1", "rev-a"), ("model-b", "2", "rev-b")] {
        let package_root = root.path().join(format!("source-{revision}"));
        std::fs::create_dir_all(&package_root).unwrap();
        write_package(&package_root, model_id, version, revision);
        let package = verify_package(&package_root, &policy()).unwrap();
        repository.install(&package, 1).await.unwrap();
    }
    repository
}

#[test]
fn package_verifier_rejects_untrusted_and_malformed_inputs() {
    let root = TestRoot::new("bad-package");
    write_package(root.path(), "model-a", "1", "rev-a");
    assert!(verify_package(root.path(), &policy()).is_ok());

    let model_path = root.path().join("model/model.bin");
    std::fs::write(&model_path, b"fake-modex").unwrap();
    assert_eq!(
        verify_package(root.path(), &policy()).unwrap_err().code,
        "model_file_hash_mismatch"
    );
    std::fs::write(&model_path, b"fake-model").unwrap();

    let manifest_path = root.path().join("manifest.yaml");
    let valid = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, valid.replace("gmv.ai/v1", "gmv.ai/v999")).unwrap();
    assert_eq!(
        verify_package(root.path(), &policy()).unwrap_err().code,
        "unsupported_model_schema"
    );
    std::fs::write(&manifest_path, &valid).unwrap();

    let expected_path = root.path().join("tests/expected.json");
    std::fs::remove_file(&expected_path).unwrap();
    assert!(verify_package(root.path(), &policy()).is_err());
    std::fs::write(&expected_path, br#"{"ok":true}"#).unwrap();

    let schema_path = root.path().join("schema/result.schema.json");
    let valid_schema = std::fs::read(&schema_path).unwrap();
    let invalid_schema = b"not valid json!!!";
    assert_eq!(valid_schema.len(), invalid_schema.len());
    std::fs::write(&schema_path, invalid_schema).unwrap();
    let schema_manifest = valid.replace(
        &format!("{:x}", Sha256::digest(&valid_schema)),
        &format!("{:x}", Sha256::digest(invalid_schema)),
    );
    std::fs::write(&manifest_path, resign_manifest(&schema_manifest)).unwrap();
    assert_eq!(
        verify_package(root.path(), &policy()).unwrap_err().code,
        "invalid_result_schema"
    );
    std::fs::write(&schema_path, valid_schema).unwrap();
    std::fs::write(&manifest_path, &valid).unwrap();

    std::fs::write(
        &manifest_path,
        valid.replace("signature: ", "signature: AA"),
    )
    .unwrap();
    assert_eq!(
        verify_package(root.path(), &policy()).unwrap_err().code,
        "model_signature_invalid"
    );
    std::fs::write(&manifest_path, &valid).unwrap();

    let mut incompatible = policy();
    incompatible.architecture = "incompatible-arch".to_string();
    assert_eq!(
        verify_package(root.path(), &incompatible).unwrap_err().code,
        "model_runtime_incompatible"
    );
    let mut no_license = policy();
    no_license.approved_spdx.clear();
    assert_eq!(
        verify_package(root.path(), &no_license).unwrap_err().code,
        "model_license_unavailable"
    );
    let mut too_small = policy();
    too_small.max_memory_mb = 32;
    assert_eq!(
        verify_package(root.path(), &too_small).unwrap_err().code,
        "model_resource_limit_exceeded"
    );

    let unsafe_manifest = valid.replace("model/model.bin", "../model.bin");
    std::fs::write(&manifest_path, resign_manifest(&unsafe_manifest)).unwrap();
    assert_eq!(
        verify_package(root.path(), &policy()).unwrap_err().code,
        "model_path_invalid"
    );
}

#[tokio::test]
async fn repository_installs_immutable_revision_and_persists_state() {
    let root = TestRoot::new("repository");
    let repository = repository_with_models(&root).await;
    let model_a = identity("model-a", "1", "rev-a");
    let installed = repository.get(&model_a).await.unwrap().unwrap();
    assert_eq!(installed.state, ModelState::Installed);
    assert!(installed.installed_path.join("model/model.bin").is_file());

    let source = root.path().join("source-rev-a");
    let package = verify_package(&source, &policy()).unwrap();
    let repeated = repository.install(&package, 2).await.unwrap();
    assert_eq!(repeated.installed_path, installed.installed_path);
    let model_b = identity("model-b", "2", "rev-b");
    let model_b_path = repository
        .get(&model_b)
        .await
        .unwrap()
        .unwrap()
        .installed_path;
    repository.remove(&model_b).await.unwrap();
    assert!(!model_b_path.exists());
    assert!(repository.get(&model_b).await.unwrap().is_none());
    repository.close().await;

    let reopened = ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
        .await
        .unwrap();
    assert_eq!(reopened.list().await.unwrap().len(), 1);
    reopened.close().await;
}

#[tokio::test]
async fn atomic_switch_keeps_in_flight_tasks_on_captured_generation() {
    let root = TestRoot::new("atomic-switch");
    let repository = repository_with_models(&root).await;
    let fake = FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior {
            block_inference: true,
            ..FakeRuntimeBehavior::default()
        },
    );
    let providers: Vec<Arc<dyn RuntimeProvider>> = vec![Arc::new(fake.clone())];
    let manager = ModelManager::open(repository.clone(), providers, ModelManagerConfig::default())
        .await
        .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 10).await.unwrap();
    manager.preload(&model_b, 11).await.unwrap();
    let generation_a = manager.activate(&model_a, 12).await.unwrap();

    let mut tasks = Vec::new();
    for _ in 0..100 {
        let active = manager.capture(CAPABILITY).await.unwrap();
        tasks.push(tokio::spawn(
            async move { active.infer(vec![1]).await.unwrap() },
        ));
    }
    for _ in 0..100 {
        if fake.started_inferences() == 100 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fake.started_inferences(), 100);
    let generation_b = manager.activate(&model_b, 13).await.unwrap();
    assert!(generation_b > generation_a);
    let new_task = manager.capture(CAPABILITY).await.unwrap();
    assert_eq!(new_task.identity(), &model_b);
    fake.release_inferences();
    for task in tasks {
        let result = task.await.unwrap();
        assert_eq!(result.actual_model.model_id, "model-a");
    }
    drop(new_task);
    assert_eq!(
        manager.retire_previous(CAPABILITY, 14).await.unwrap(),
        Some(model_a.clone())
    );
    manager.unload(&model_a, 15).await.unwrap();
    assert!(
        manager
            .status()
            .await
            .iter()
            .all(|status| status.identity != model_a)
    );
    repository.close().await;
}

#[tokio::test]
async fn failed_candidate_does_not_replace_active_and_rollback_is_explicit() {
    let root = TestRoot::new("failure-rollback");
    let repository = repository_with_models(&root).await;
    let good: Vec<Arc<dyn RuntimeProvider>> = vec![Arc::new(FakeRuntimeProvider::new(
        "fake",
        FakeRuntimeBehavior::default(),
    ))];
    let manager = ModelManager::open(
        repository.clone(),
        good,
        ModelManagerConfig {
            max_loaded_models: 2,
            max_memory_mb: 127,
            max_vram_mb: 1,
        },
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 20).await.unwrap();
    let generation_a = manager.activate(&model_a, 21).await.unwrap();
    assert_eq!(manager.activate(&model_a, 22).await.unwrap(), generation_a);
    assert_eq!(
        manager.preload(&model_b, 23).await.unwrap_err().code,
        "model_runtime_budget_exceeded"
    );
    assert_eq!(
        manager.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );

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
    manager.preload(&model_a, 24).await.unwrap();
    manager.preload(&model_b, 25).await.unwrap();
    manager.activate(&model_a, 26).await.unwrap();
    manager.activate(&model_b, 27).await.unwrap();
    let rollback_generation = manager.rollback(CAPABILITY, 28).await.unwrap();
    let restored = manager.capture(CAPABILITY).await.unwrap();
    assert_eq!(restored.identity(), &model_a);
    assert_eq!(restored.generation(), rollback_generation);
    drop(restored);
    drop(manager);

    let restarted = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior::default(),
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let restored = restarted.capture(CAPABILITY).await.unwrap();
    assert_eq!(restored.identity(), &model_a);
    assert_eq!(restored.generation(), rollback_generation);
    drop(restored);
    restarted.preload(&model_b, 29).await.unwrap();
    assert!(restarted.activate(&model_b, 30).await.unwrap() > rollback_generation);
    repository.close().await;
}

#[tokio::test]
async fn preload_and_self_test_failure_leave_existing_active_model_unchanged() {
    for behavior in [
        FakeRuntimeBehavior {
            fail_preload_model: Some("model-b".to_string()),
            ..FakeRuntimeBehavior::default()
        },
        FakeRuntimeBehavior {
            fail_self_test_model: Some("model-b".to_string()),
            ..FakeRuntimeBehavior::default()
        },
    ] {
        let root = TestRoot::new("runtime-failure");
        let repository = repository_with_models(&root).await;
        let manager = ModelManager::open(
            repository.clone(),
            vec![Arc::new(FakeRuntimeProvider::new("fake", behavior))],
            ModelManagerConfig::default(),
        )
        .await
        .unwrap();
        let model_a = identity("model-a", "1", "rev-a");
        let model_b = identity("model-b", "2", "rev-b");
        manager.preload(&model_a, 30).await.unwrap();
        manager.activate(&model_a, 31).await.unwrap();
        assert!(manager.preload(&model_b, 32).await.is_err());
        assert_eq!(
            manager.capture(CAPABILITY).await.unwrap().identity(),
            &model_a
        );
        repository.close().await;
    }
}
