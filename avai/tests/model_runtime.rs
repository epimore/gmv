use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use avai::model::{
    FakeRuntimeBehavior, FakeRuntimeProvider, HealthReconcile, ModelIdentity, ModelManager,
    ModelManagerConfig, ModelPackageManifest, ModelRepository, ModelState, PackagePolicy,
    RuntimeProvider, model_package_signing_payload, verify_package,
};
use base::{
    base64::Engine,
    sha2::{Digest, Sha256},
};
use ed25519_dalek::{Signer, SigningKey};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);
const CAPABILITY: &str = "vehicle.detect";
const SECOND_CAPABILITY: &str = "vision.object.detect";

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
    write_package_with_capabilities(root, model_id, version, revision, &[CAPABILITY]);
}

fn write_package_with_capabilities(
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
    let unsigned_manifest = format!(
        "api_version: gmv.ai/v1\nkind: ModelPlugin\nmetadata:\n  model_id: {model_id}\n  version: {version}\n  revision: {revision}\ncapabilities:\n{capabilities_yaml}\nresult_schema:\n  name: gmv.vision.observation\n  version: 1\n  path: schema/result.schema.json\nvariants:\n  - runtime: fake\n    architecture: {}\n    accelerator: cpu\n    artifact: model/model.bin\nresources:\n  memory_mb: 64\n  vram_mb: 0\n  max_batch: 4\nlicense:\n  spdx: Apache-2.0\n  commercial_use: true\n  redistribution: allowed\n  license_ref: \"\"\nself_test:\n  - input: tests/input.bin\n    expected: tests/expected.json\nfiles:\n{file_yaml}\nsigning:\n  key_id: test-key\n  signature: \"\"\n",
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

async fn install_test_model(
    repository: &ModelRepository,
    root: &TestRoot,
    model_id: &str,
    version: &str,
    revision: &str,
    capabilities: &[&str],
) {
    let package_root = root.path().join(format!("source-{revision}"));
    std::fs::create_dir_all(&package_root).unwrap();
    write_package_with_capabilities(&package_root, model_id, version, revision, capabilities);
    let package = verify_package(&package_root, &policy()).unwrap();
    repository.install(&package, 1).await.unwrap();
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
async fn install_rejects_package_mutated_after_verification() {
    let root = TestRoot::new("install-toctou");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, "model-a", "1", "rev-a");
    let package = verify_package(&source, &policy()).unwrap();
    std::fs::write(source.join("model/model.bin"), b"fake-modex").unwrap();
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    let model_a = identity("model-a", "1", "rev-a");

    assert_eq!(
        repository.install(&package, 1).await.unwrap_err().code,
        "model_file_hash_mismatch"
    );
    assert!(repository.get(&model_a).await.unwrap().is_none());
    assert!(!root.path().join("models/packages/model-a/1/rev-a").exists());
    repository.close().await;
}

#[tokio::test]
async fn install_recovers_crash_after_rename_before_database_commit() {
    let root = TestRoot::new("install-rename-crash");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, "model-a", "1", "rev-a");
    let package = verify_package(&source, &policy()).unwrap();
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    let destination = root.path().join("models/packages/model-a/1/rev-a");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();

    std::fs::rename(&source, &destination).unwrap();
    assert!(
        repository
            .get(&identity("model-a", "1", "rev-a"))
            .await
            .unwrap()
            .is_none()
    );

    let installed = repository.install(&package, 2).await.unwrap();
    assert_eq!(installed.installed_path, destination);
    assert_eq!(repository.list().await.unwrap().len(), 1);
    assert!(installed.installed_path.join("model/model.bin").is_file());
    repository.close().await;
}

#[tokio::test]
async fn install_rebuilds_unverified_orphan_instead_of_accepting_it() {
    let root = TestRoot::new("install-invalid-orphan");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    write_package(&source, "model-a", "1", "rev-a");
    let package = verify_package(&source, &policy()).unwrap();
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    let destination = root.path().join("models/packages/model-a/1/rev-a");
    std::fs::create_dir_all(&destination).unwrap();
    write_package(&destination, "model-a", "1", "rev-a");
    std::fs::write(destination.join("model/model.bin"), b"unverified").unwrap();

    let installed = repository.install(&package, 2).await.unwrap();
    assert_eq!(
        std::fs::read(installed.installed_path.join("model/model.bin")).unwrap(),
        b"fake-model"
    );
    assert_eq!(repository.list().await.unwrap().len(), 1);
    assert_eq!(
        std::fs::read_dir(root.path().join("models/quarantine"))
            .unwrap()
            .count(),
        0
    );
    repository.close().await;
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
    assert_eq!(
        manager
            .retire_previous(CAPABILITY, 14)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );
    assert_eq!(fake.dropped_instances("model-a"), 0);
    fake.release_inferences();
    for task in tasks {
        let result = task.await.unwrap();
        assert_eq!(result.actual_model.model_id, "model-a");
    }
    drop(new_task);
    assert_eq!(fake.dropped_instances("model-a"), 0);
    assert_eq!(
        manager.retire_previous(CAPABILITY, 15).await.unwrap(),
        Some(model_a.clone())
    );
    manager.unload(&model_a, 16).await.unwrap();
    assert_eq!(fake.dropped_instances("model-a"), 1);
    manager.unload(&model_a, 17).await.unwrap();
    assert_eq!(fake.dropped_instances("model-a"), 1);
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
async fn activate_and_unload_share_one_lifecycle_boundary() {
    let root = TestRoot::new("activate-unload-race");
    let repository = repository_with_models(&root).await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 10).await.unwrap();
    manager.preload(&model_b, 11).await.unwrap();
    manager.activate(&model_a, 12).await.unwrap();

    let health_before = fake.started_health_checks();
    fake.pause_health_checks();
    let activate_manager = manager.clone();
    let activate_model = model_b.clone();
    let activate =
        tokio::spawn(async move { activate_manager.activate(&activate_model, 13).await });
    while fake.started_health_checks() == health_before {
        tokio::task::yield_now().await;
    }
    let unload_manager = manager.clone();
    let unload_model = model_b.clone();
    let unload = tokio::spawn(async move { unload_manager.unload(&unload_model, 14).await });
    tokio::task::yield_now().await;
    assert!(!unload.is_finished());
    fake.release_health_checks();

    assert!(activate.await.unwrap().is_ok());
    assert_eq!(unload.await.unwrap().unwrap_err().code, "model_in_use");
    assert_eq!(
        manager.capture(CAPABILITY).await.unwrap().identity(),
        &model_b
    );
    assert_eq!(
        repository.get(&model_b).await.unwrap().unwrap().state,
        ModelState::Active
    );
    assert!(
        manager
            .status()
            .await
            .iter()
            .any(|status| status.identity == model_b)
    );
    repository.close().await;
}

#[tokio::test]
async fn restart_reconciles_ready_and_restores_previous_for_rollback() {
    let root = TestRoot::new("restart-slots");
    let repository = repository_with_models(&root).await;
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");

    let ready_manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior::default(),
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    ready_manager.preload(&model_a, 20).await.unwrap();
    drop(ready_manager);
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
    assert!(restarted.status().await.is_empty());
    assert_eq!(
        repository.get(&model_a).await.unwrap().unwrap().state,
        ModelState::Installed
    );

    restarted.preload(&model_a, 21).await.unwrap();
    restarted.preload(&model_b, 22).await.unwrap();
    restarted.activate(&model_a, 23).await.unwrap();
    restarted.activate(&model_b, 24).await.unwrap();
    drop(restarted);
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
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_b
    );
    restarted.rollback(CAPABILITY, 25).await.unwrap();
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    repository.close().await;
}

#[tokio::test]
async fn transient_lifecycle_states_are_never_durably_exposed() {
    let root = TestRoot::new("stable-states-only");
    let repository = repository_with_models(&root).await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");

    let health_before = fake.started_health_checks();
    fake.pause_health_checks();
    let preload_manager = manager.clone();
    let preload_model = model_a.clone();
    let preload = tokio::spawn(async move { preload_manager.preload(&preload_model, 30).await });
    while fake.started_health_checks() == health_before {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        repository.get(&model_a).await.unwrap().unwrap().state,
        ModelState::Installed
    );
    fake.release_health_checks();
    preload.await.unwrap().unwrap();

    manager.preload(&model_b, 31).await.unwrap();
    manager.activate(&model_a, 32).await.unwrap();
    let held = manager.capture(CAPABILITY).await.unwrap();
    manager.activate(&model_b, 33).await.unwrap();
    assert_eq!(
        manager
            .retire_previous(CAPABILITY, 34)
            .await
            .unwrap_err()
            .code,
        "model_in_use"
    );
    drop(held);
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
    restarted.rollback(CAPABILITY, 35).await.unwrap();
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    repository.close().await;
}

#[tokio::test]
async fn unhealthy_active_model_rolls_back_to_healthy_previous() {
    let root = TestRoot::new("health-rollback");
    let repository = repository_with_models(&root).await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 40).await.unwrap();
    manager.preload(&model_b, 41).await.unwrap();
    manager.activate(&model_a, 42).await.unwrap();
    manager.activate(&model_b, 43).await.unwrap();
    fake.set_model_unhealthy("model-b", true);

    let result = manager
        .reconcile_active_health(CAPABILITY, 44)
        .await
        .unwrap();
    assert!(matches!(
        &result,
        HealthReconcile::RolledBack {
            failed,
            restored,
            cleared_capabilities,
        } if failed == &model_b
            && restored.len() == 1
            && restored[0].identity == model_a
            && cleared_capabilities.is_empty()
    ));
    assert_eq!(
        manager.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    assert_eq!(
        repository.get(&model_b).await.unwrap().unwrap().state,
        ModelState::Failed
    );
    assert!(!manager.active_identities().await.contains(&model_b));
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
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    repository.close().await;
}

#[tokio::test]
async fn unhealthy_multi_capability_model_recovers_every_active_slot() {
    let root = TestRoot::new("multi-capability-health-rollback");
    let repository =
        ModelRepository::open(&root.path().join("avai.db"), &root.path().join("models"))
            .await
            .unwrap();
    install_test_model(&repository, &root, "model-a", "1", "rev-a", &[CAPABILITY]).await;
    install_test_model(
        &repository,
        &root,
        "model-c",
        "1",
        "rev-c",
        &[SECOND_CAPABILITY],
    )
    .await;
    install_test_model(
        &repository,
        &root,
        "model-b",
        "2",
        "rev-b",
        &[CAPABILITY, SECOND_CAPABILITY],
    )
    .await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    let model_c = identity("model-c", "1", "rev-c");
    for model in [&model_a, &model_c, &model_b] {
        manager.preload(model, 50).await.unwrap();
    }
    manager.activate(&model_a, 51).await.unwrap();
    manager.activate(&model_c, 52).await.unwrap();
    manager.activate(&model_b, 53).await.unwrap();
    fake.set_model_unhealthy("model-b", true);

    let result = manager
        .reconcile_active_health(CAPABILITY, 54)
        .await
        .unwrap();
    let HealthReconcile::RolledBack {
        failed,
        restored,
        cleared_capabilities,
    } = result
    else {
        panic!("expected model rollback");
    };
    assert_eq!(failed, model_b);
    assert!(cleared_capabilities.is_empty());
    assert_eq!(restored.len(), 2);
    assert!(
        restored
            .iter()
            .any(|recovery| { recovery.capability == CAPABILITY && recovery.identity == model_a })
    );
    assert!(restored.iter().any(|recovery| {
        recovery.capability == SECOND_CAPABILITY && recovery.identity == model_c
    }));
    assert_eq!(
        manager.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    assert_eq!(
        manager.capture(SECOND_CAPABILITY).await.unwrap().identity(),
        &model_c
    );
    assert_eq!(
        repository.get(&model_b).await.unwrap().unwrap().state,
        ModelState::Failed
    );
    assert_eq!(
        manager.active_identities().await,
        HashSet::from([model_a.clone(), model_c.clone()])
    );
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
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    assert_eq!(
        restarted
            .capture(SECOND_CAPABILITY)
            .await
            .unwrap()
            .identity(),
        &model_c
    );
    repository.close().await;
}

#[tokio::test]
async fn restart_falls_back_when_persisted_active_is_unhealthy() {
    let root = TestRoot::new("startup-health-fallback");
    let repository = repository_with_models(&root).await;
    let fake = FakeRuntimeProvider::new("fake", FakeRuntimeBehavior::default());
    let manager = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake.clone())],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 60).await.unwrap();
    manager.preload(&model_b, 61).await.unwrap();
    manager.activate(&model_a, 62).await.unwrap();
    manager.activate(&model_b, 63).await.unwrap();
    fake.set_model_unhealthy("model-b", true);
    drop(manager);

    let restarted = ModelManager::open(
        repository.clone(),
        vec![Arc::new(fake)],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        restarted.capture(CAPABILITY).await.unwrap().identity(),
        &model_a
    );
    assert_eq!(
        repository.get(&model_a).await.unwrap().unwrap().state,
        ModelState::Active
    );
    assert_eq!(
        repository.get(&model_b).await.unwrap().unwrap().state,
        ModelState::Failed
    );
    repository.close().await;
}

#[tokio::test]
async fn restart_reports_stable_error_when_active_and_previous_both_fail() {
    let root = TestRoot::new("startup-recovery-failure");
    let repository = repository_with_models(&root).await;
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
    let model_a = identity("model-a", "1", "rev-a");
    let model_b = identity("model-b", "2", "rev-b");
    manager.preload(&model_a, 70).await.unwrap();
    manager.preload(&model_b, 71).await.unwrap();
    manager.activate(&model_a, 72).await.unwrap();
    manager.activate(&model_b, 73).await.unwrap();
    drop(manager);

    let error = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior {
                fail_preload: true,
                ..FakeRuntimeBehavior::default()
            },
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "model_startup_recovery_failed");
    assert_eq!(
        repository.get(&model_b).await.unwrap().unwrap().state,
        ModelState::Active
    );
    assert_eq!(
        repository.get(&model_a).await.unwrap().unwrap().state,
        ModelState::Ready
    );

    let retried = ModelManager::open(
        repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior::default(),
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        retried.capture(CAPABILITY).await.unwrap().identity(),
        &model_b
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
