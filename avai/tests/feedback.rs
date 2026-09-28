use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use base::{
    bytes::Bytes,
    futures::StreamExt,
    sha2::{Digest, Sha256},
};
use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{self, Row},
};
use gmv_protocol::{
    avai::{
        feedback::v1::{self as rpc, avai_feedback_server::AvaiFeedback},
        v1::{
            AiTaskResult, ImageUrlSource, ModelRef, SourceSpec, VersionedPayload,
            source_spec::Source,
        },
    },
    common::v1::NodeIdentity,
};
use prost::Message;
use tonic::Request;

use crate::feedback::{
    AvaiFeedbackRpc, FeedbackConfig, FeedbackManager, FeedbackMaterial, safe_source_ref, sampled,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    database: PathBuf,
    manager: FeedbackManager,
    config: FeedbackConfig,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn result() -> AiTaskResult {
    AiTaskResult {
        output: Some(VersionedPayload {
            schema: "test.schema".into(),
            version: 1,
            json: br#"{"ok":true}"#.to_vec(),
        }),
        actual_model: Some(ModelRef {
            model_id: "model".into(),
            version: "1".into(),
            runtime: "rust".into(),
            revision: String::new(),
        }),
        evidence: vec![],
        completed_at_epoch_ms: 1_800_000_000_000,
    }
}

fn material(bytes: &[u8]) -> FeedbackMaterial {
    FeedbackMaterial {
        task_id: "task-1".into(),
        request_hash: "request-hash".into(),
        route_id: "route-1".into(),
        capability: "image.metadata.inspect".into(),
        source_ref: "https://example.com/image.jpg".into(),
        result: result(),
        evidence: Bytes::copy_from_slice(bytes),
        evidence_sha256: format!("{:x}", Sha256::digest(bytes)),
        evidence_media_type: "image/jpeg".into(),
    }
}

async fn fixture() -> Fixture {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-feedback-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("task.db");
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("CREATE TABLE avai_task(task_id TEXT PRIMARY KEY, state INTEGER, execution_binding BLOB, result BLOB, capability TEXT, route_id TEXT)")
        .execute(&pool).await.unwrap();
    let binding = br#"{"capability":"image.metadata.inspect","result_schema_name":"test.schema","result_schema_version":1,"model_id":"model","model_version":"1","revision":"","runtime":"rust"}"#;
    sqlx::query("INSERT INTO avai_task(task_id,state,execution_binding,result,capability,route_id) VALUES(?,?,?,?,?,?)")
        .bind("task-1").bind(2i32).bind(binding.as_slice()).bind(result().encode_to_vec())
        .bind("image.metadata.inspect").bind("route-1").execute(&pool).await.unwrap();
    drop(pool);
    let config = FeedbackConfig {
        enabled: true,
        spool_root: root.join("spool"),
        capabilities: HashSet::from(["image.metadata.inspect".to_string()]),
        ..Default::default()
    };
    let manager = FeedbackManager::open(
        &database,
        config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    Fixture {
        root,
        database,
        manager,
        config,
    }
}

fn rpc(manager: &FeedbackManager) -> AvaiFeedbackRpc {
    AvaiFeedbackRpc::new(Some(manager.clone()))
}

async fn list(manager: &FeedbackManager) -> rpc::ListPendingFeedbackResponse {
    rpc(manager)
        .list_pending_feedback(Request::new(rpc::ListPendingFeedbackRequest::default()))
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn sampling_vectors_and_capability() {
    let config = FeedbackConfig {
        enabled: true,
        capabilities: HashSet::from(["inspect".into()]),
        sample_permyriad: 501,
        ..Default::default()
    };
    assert!(sampled(&config, "inspect", "task-a", "hash-a"));
    assert!(!sampled(&config, "inspect", "task-b", "hash-b"));
    assert!(!sampled(&config, "different", "task-a", "hash-a"));
    assert!(!sampled(
        &FeedbackConfig {
            sample_permyriad: 0,
            ..config.clone()
        },
        "inspect",
        "task-a",
        "hash-a"
    ));
    assert!(sampled(
        &FeedbackConfig {
            sample_permyriad: 10_000,
            ..config
        },
        "inspect",
        "task-b",
        "hash-b"
    ));
}

#[test]
fn source_ref_strips_credentials_without_persisting_proof() {
    for (url, forbidden) in [
        (
            "https://example/image.jpg?token=abc#x",
            vec!["token", "?", "#"],
        ),
        (
            "https://user:password@example/image.jpg",
            vec!["user", "password", "@"],
        ),
    ] {
        let source = SourceSpec {
            source: Some(Source::ImageUrl(ImageUrlSource {
                url: url.into(),
                expected: None,
                max_bytes: 0,
            })),
        };
        let sanitized = safe_source_ref(&source).unwrap();
        for fragment in forbidden {
            assert!(!sanitized.contains(fragment));
        }
    }
}

#[tokio::test]
async fn prepared_recovery_and_orphan_cleanup() {
    let fixture = fixture().await;
    let id = fixture
        .manager
        .prepare(material(b"evidence"))
        .await
        .unwrap()
        .unwrap();
    assert!(list(&fixture.manager).await.packages.is_empty());
    let orphan = fixture.config.spool_root.join("orphan.evidence");
    std::fs::write(&orphan, b"orphan").unwrap();
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let reopened = FeedbackManager::open(
        &fixture.database,
        fixture.config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    assert_eq!(list(&reopened).await.packages[0].feedback_id, id);
    assert!(!orphan.exists());
}

#[tokio::test]
async fn prepared_non_success_is_removed_on_restart() {
    let fixture = fixture().await;
    fixture
        .manager
        .prepare(material(b"evidence"))
        .await
        .unwrap();
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=1 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let reopened = FeedbackManager::open(
        &fixture.database,
        fixture.config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    assert!(list(&reopened).await.packages.is_empty());
    assert_eq!(
        std::fs::read_dir(&fixture.config.spool_root)
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn rpc_stream_identity_cancel_and_ack_replay() {
    let fixture = fixture().await;
    let bytes = vec![42u8; 150_000];
    let id = fixture
        .manager
        .prepare(material(&bytes))
        .await
        .unwrap()
        .unwrap();
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    fixture.manager.promote(&id).await.unwrap();
    let service = rpc(&fixture.manager);
    let capabilities = service
        .get_feedback_capabilities(Request::new(rpc::GetFeedbackCapabilitiesRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert!(capabilities.enabled);
    assert_eq!(capabilities.max_evidence_chunk_bytes, 65_536);
    let package = list(&fixture.manager).await.packages.remove(0);
    assert_eq!(package.feedback_id, id);
    assert!(
        !package
            .encode_to_vec()
            .windows(8)
            .any(|window| window == b"********")
    );
    let request = rpc::ReadFeedbackEvidenceRequest {
        feedback_id: id.clone(),
        content_hash: package.content_hash.clone(),
        evidence_sha256: package.evidence_sha256.clone(),
        expected_size: bytes.len() as u64,
    };
    for mutant in [
        rpc::ReadFeedbackEvidenceRequest {
            content_hash: "wrong".into(),
            ..request.clone()
        },
        rpc::ReadFeedbackEvidenceRequest {
            evidence_sha256: "wrong".into(),
            ..request.clone()
        },
        rpc::ReadFeedbackEvidenceRequest {
            expected_size: 1,
            ..request.clone()
        },
        rpc::ReadFeedbackEvidenceRequest {
            feedback_id: "wrong".into(),
            ..request.clone()
        },
    ] {
        assert!(
            service
                .read_feedback_evidence(Request::new(mutant))
                .await
                .is_err()
        );
    }
    let mut partial = service
        .read_feedback_evidence(Request::new(request.clone()))
        .await
        .unwrap()
        .into_inner();
    assert!(partial.next().await.unwrap().unwrap().chunk.len() <= 65_536);
    drop(partial);
    assert_eq!(list(&fixture.manager).await.packages.len(), 1);
    let mut full = service
        .read_feedback_evidence(Request::new(request))
        .await
        .unwrap()
        .into_inner();
    let mut received = Vec::new();
    while let Some(part) = full.next().await {
        received.extend(part.unwrap().chunk);
    }
    assert_eq!(received, bytes);
    let ack = rpc::AckFeedbackRequest {
        feedback_id: id.clone(),
        content_hash: package.content_hash.clone(),
        terminal_outcome: rpc::FeedbackTerminalOutcome::Accepted as i32,
    };
    assert!(
        !service
            .ack_feedback(Request::new(ack.clone()))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
    assert!(list(&fixture.manager).await.packages.is_empty());
    assert!(
        service
            .ack_feedback(Request::new(ack.clone()))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
    assert!(
        service
            .ack_feedback(Request::new(rpc::AckFeedbackRequest {
                terminal_outcome: rpc::FeedbackTerminalOutcome::Conflict as i32,
                ..ack
            }))
            .await
            .is_err()
    );
    assert!(
        service
            .ack_feedback(Request::new(rpc::AckFeedbackRequest {
                feedback_id: id,
                content_hash: package.content_hash,
                terminal_outcome: 0
            }))
            .await
            .is_err()
    );
    drop(pool);
}
async fn image_server(bytes: Vec<u8>) -> (String, base::tokio::task::JoinHandle<()>) {
    use base::tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let task = base::tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = connection.read(&mut request).await.unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        connection.write_all(header.as_bytes()).await.unwrap();
        connection.write_all(&bytes).await.unwrap();
    });
    (
        format!("http://{address}/image.png?token=abc#fragment"),
        task,
    )
}

#[tokio::test]
async fn successful_inference_prepares_feedback_without_consumer() {
    use crate::{
        observability::Observability,
        source::SourcePolicy,
        task::{TaskManager, TaskManagerConfig},
    };
    use base::utils::rt::{GlobalRuntime, RuntimeType};
    use gmv_protocol::{
        avai::v1::{AiTaskState, CreateTaskRequest, QueryTaskRequest},
        common::v1::OperationRef,
    };
    use std::{
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    let test_id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "avai-feedback-inference-{}-{test_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = GlobalRuntime::register_default(RuntimeType::Custom(format!(
        "avai-feedback-inference-{test_id}"
    )))
    .unwrap();
    let identity = NodeIdentity {
        node_id: "node".into(),
        instance_id: "instance".into(),
        kind: 4,
    };
    let feedback_config = FeedbackConfig {
        enabled: true,
        spool_root: root.join("feedback"),
        capabilities: HashSet::from(["image.metadata.inspect".into()]),
        sample_permyriad: 10_000,
        max_pending_items: 2,
        ..Default::default()
    };
    let manager = TaskManager::open_with_feedback(
        identity.clone(),
        vec!["image.metadata.inspect".into()],
        TaskManagerConfig {
            database_path: root.join("task.db"),
            worker_count: 1,
            queue_size: 4,
            source_policy: SourcePolicy {
                allow_private_image_urls: true,
                ..Default::default()
            },
            max_result_bytes: 1024,
        },
        None,
        &runtime,
        Arc::new(Observability::new()),
        Some((feedback_config, "installation".into(), "host".into())),
    )
    .await
    .unwrap();
    let feedback = manager.feedback_manager().unwrap();
    let png = base::base64::Engine::decode(&base::base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    for (ordinal, hold_lane) in [(1, false), (2, true), (3, false), (4, false), (5, false)] {
        if ordinal == 5 {
            let pending = list(&feedback).await.packages;
            assert_eq!(pending.len(), 2);
            assert_eq!(pending[0].task_id, "task-1");
            assert_eq!(pending[1].task_id, "task-3");
            for package in pending {
                rpc(&feedback)
                    .ack_feedback(Request::new(rpc::AckFeedbackRequest {
                        feedback_id: package.feedback_id,
                        content_hash: package.content_hash,
                        terminal_outcome: rpc::FeedbackTerminalOutcome::Accepted as i32,
                    }))
                    .await
                    .unwrap();
            }
            std::fs::remove_dir(&feedback.config().spool_root).unwrap();
            std::fs::write(&feedback.config().spool_root, b"not a directory").unwrap();
        }
        let (url, server) = image_server(png.clone()).await;
        let task_id = format!("task-{ordinal}");
        let request = CreateTaskRequest {
            operation: Some(OperationRef {
                operation_id: format!("op-{ordinal}"),
                idempotency_key: format!("idem-{ordinal}"),
            }),
            task_id: task_id.clone(),
            route_id: "route".into(),
            capability: "image.metadata.inspect".into(),
            expected_avai: Some(identity.clone()),
            source: Some(SourceSpec {
                source: Some(Source::ImageUrl(ImageUrlSource {
                    url,
                    expected: None,
                    max_bytes: 1024,
                })),
            }),
            ..Default::default()
        };
        let guard = if hold_lane {
            Some(feedback.hold_lane_for_test().await)
        } else {
            None
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert_eq!(
            manager.create_task(request, now).await.state,
            AiTaskState::Pending as i32
        );
        let task_state = base::tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = manager
                    .query_task(QueryTaskRequest {
                        task_id: task_id.clone(),
                    })
                    .await
                    .state;
                if state != AiTaskState::Pending as i32 && state != AiTaskState::Running as i32 {
                    break state;
                }
                base::tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(task_state, AiTaskState::Succeeded as i32);
        drop(guard);
        server.await.unwrap();
    }
    assert!(list(&feedback).await.packages.is_empty());
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn capacity_conflict_expiry_and_receipt_bound() {
    let fixture = fixture().await;
    let mut config = fixture.config.clone();
    config.max_pending_items = 1;
    config.max_total_bytes = 8;
    config.ack_receipt_capacity = 1;
    let manager = FeedbackManager::open(
        &fixture.database,
        config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    let first = material(b"123456");
    let id = manager.prepare(first).await.unwrap().unwrap();
    assert_eq!(
        manager.prepare(material(b"123456")).await.unwrap(),
        Some(id.clone())
    );
    assert_eq!(
        manager
            .prepare(material(b"changed"))
            .await
            .unwrap_err()
            .code,
        "feedback_identity_conflict"
    );
    let mut other = material(b"1");
    other.task_id = "task-2".into();
    assert_eq!(
        manager.prepare(other).await.unwrap_err().code,
        "feedback_spool_full"
    );
    manager.discard(&id).await.unwrap();
    let mut byte_config = config.clone();
    byte_config.max_pending_items = 3;
    let manager = FeedbackManager::open(
        &fixture.database,
        byte_config,
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    let first = manager.prepare(material(b"123456")).await.unwrap().unwrap();
    let mut other = material(b"123");
    other.task_id = "task-2".into();
    assert_eq!(
        manager.prepare(other).await.unwrap_err().code,
        "feedback_spool_full"
    );
    manager.discard(&first).await.unwrap();

    let mut expiry = config;
    expiry.ttl_ms = 1;
    let manager = FeedbackManager::open(
        &fixture.database,
        expiry,
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    let mut stale = material(b"stale");
    stale.result.completed_at_epoch_ms = 1;
    manager.prepare(stale).await.unwrap();
    assert!(list(&manager).await.packages.is_empty());
    manager.sweep().await.unwrap();
    assert_eq!(
        std::fs::read_dir(&fixture.config.spool_root)
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn evidence_mismatch_fails_closed_and_restart_drops_candidate() {
    let fixture = fixture().await;
    let id = fixture
        .manager
        .prepare(material(b"genuine"))
        .await
        .unwrap()
        .unwrap();
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    fixture.manager.promote(&id).await.unwrap();
    let package = list(&fixture.manager).await.packages.remove(0);
    let path = fixture
        .config
        .spool_root
        .join(format!("{}_{}.evidence", id, package.evidence_sha256));
    std::fs::write(&path, b"forgery").unwrap();
    let mut stream = rpc(&fixture.manager)
        .read_feedback_evidence(Request::new(rpc::ReadFeedbackEvidenceRequest {
            feedback_id: id.clone(),
            content_hash: package.content_hash,
            evidence_sha256: package.evidence_sha256,
            expected_size: 7,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stream.next().await.unwrap().unwrap().chunk, b"forgery");
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
    assert_eq!(list(&fixture.manager).await.packages.len(), 1);
    let reopened = FeedbackManager::open(
        &fixture.database,
        fixture.config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    assert!(list(&reopened).await.packages.is_empty());
    assert!(!path.exists());
    drop(pool);
}

#[tokio::test]
async fn feedback_service_works_over_local_uds() {
    use base::tokio_util::sync::CancellationToken;
    use gmv_nodec::component_management::OwnedUdsListener;
    use gmv_protocol::avai::feedback::v1::{
        avai_feedback_client::AvaiFeedbackClient, avai_feedback_server::AvaiFeedbackServer,
    };
    use tonic::transport::Endpoint;

    let fixture = fixture().await;
    let socket = fixture.root.join("feedback.sock");
    let owned = OwnedUdsListener::bind(&socket).await.unwrap();
    let incoming = owned.incoming();
    let cancel = CancellationToken::new();
    let shutdown = cancel.clone();
    let service = rpc(&fixture.manager);
    let server = base::tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AvaiFeedbackServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move { shutdown.cancelled().await })
            .await
            .unwrap();
        owned.cleanup().unwrap();
    });
    let channel = Endpoint::try_from(format!("unix://{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = AvaiFeedbackClient::new(channel);
    let capabilities = client
        .get_feedback_capabilities(rpc::GetFeedbackCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(capabilities.contract_version, 1);
    assert_eq!(
        capabilities.supported_trigger_kinds,
        vec![rpc::FeedbackTriggerKind::Sampled as i32]
    );
    assert!(
        client
            .list_pending_feedback(rpc::ListPendingFeedbackRequest::default())
            .await
            .unwrap()
            .into_inner()
            .packages
            .is_empty()
    );
    cancel.cancel();
    server.await.unwrap();
}
#[tokio::test]
async fn ack_receipts_are_capacity_and_retention_bounded() {
    let fixture = fixture().await;
    let mut config = fixture.config.clone();
    config.ack_receipt_capacity = 1;
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO avai_task SELECT 'task-2',state,execution_binding,result,capability,route_id FROM avai_task WHERE task_id='task-1'")
        .execute(&pool).await.unwrap();
    let manager = FeedbackManager::open(
        &fixture.database,
        config,
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    let service = rpc(&manager);
    for task_id in ["task-1", "task-2"] {
        let mut candidate = material(b"evidence");
        candidate.task_id = task_id.into();
        let id = manager.prepare(candidate).await.unwrap().unwrap();
        manager.promote(&id).await.unwrap();
        let package = list(&manager)
            .await
            .packages
            .into_iter()
            .find(|p| p.task_id == task_id)
            .unwrap();
        let request = rpc::AckFeedbackRequest {
            feedback_id: id,
            content_hash: package.content_hash,
            terminal_outcome: rpc::FeedbackTerminalOutcome::Accepted as i32,
        };
        assert!(
            !service
                .ack_feedback(Request::new(request.clone()))
                .await
                .unwrap()
                .into_inner()
                .replayed
        );
        assert!(
            service
                .ack_feedback(Request::new(request))
                .await
                .unwrap()
                .into_inner()
                .replayed
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM avai_feedback_ack_receipt")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    sqlx::query("UPDATE avai_feedback_ack_receipt SET expires_at_ms=1")
        .execute(&pool)
        .await
        .unwrap();
    manager.sweep().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM avai_feedback_ack_receipt")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn stable_pagination_and_feedback_rows_exclude_grant_proof() {
    use gmv_protocol::{
        avai::v1::OwnedImageRef,
        common::v1::{AccessGrant, ResourceRef},
    };
    let fixture = fixture().await;
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    let proof = b"private-proof-unique-12345";
    let source = SourceSpec {
        source: Some(Source::OwnedImage(OwnedImageRef {
            owner: Some(NodeIdentity {
                node_id: "node".into(),
                instance_id: "instance".into(),
                kind: 4,
            }),
            resource: Some(ResourceRef {
                resource_type: "image".into(),
                resource_id: "item".into(),
            }),
            metadata: None,
            access: Some(AccessGrant {
                proof: proof.to_vec(),
                ..Default::default()
            }),
        })),
    };
    let source_ref = safe_source_ref(&source).unwrap();
    assert_eq!(source_ref, "owned:node/image/item");
    let mut expected_ids = Vec::new();
    for index in 1..=3 {
        let task_id = format!("task-{index}");
        if index > 1 {
            sqlx::query("INSERT INTO avai_task SELECT ?,state,execution_binding,result,capability,route_id FROM avai_task WHERE task_id='task-1'")
                .bind(&task_id).execute(&pool).await.unwrap();
        }
        let mut candidate = material(b"evidence");
        candidate.task_id = task_id;
        candidate.source_ref = source_ref.clone();
        let id = fixture.manager.prepare(candidate).await.unwrap().unwrap();
        fixture.manager.promote(&id).await.unwrap();
        expected_ids.push(id);
    }
    expected_ids.sort();
    let rows = sqlx::query("SELECT package FROM avai_feedback_outbox")
        .fetch_all(&pool)
        .await
        .unwrap();
    for row in rows {
        let bytes: Vec<u8> = row.try_get("package").unwrap();
        assert!(!bytes.windows(proof.len()).any(|window| window == proof));
    }
    let service = rpc(&fixture.manager);
    let mut token = String::new();
    let mut ids = Vec::new();
    loop {
        let page = service
            .list_pending_feedback(Request::new(rpc::ListPendingFeedbackRequest {
                page_size: 1,
                page_token: token,
            }))
            .await
            .unwrap()
            .into_inner();
        ids.extend(page.packages.into_iter().map(|package| package.feedback_id));
        token = page.next_page_token;
        if token.is_empty() {
            break;
        }
    }
    assert_eq!(ids, expected_ids);
    assert!(
        service
            .list_pending_feedback(Request::new(rpc::ListPendingFeedbackRequest {
                page_size: 101,
                page_token: String::new()
            }))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn management_uds_mounts_feedback_with_existing_services() {
    use crate::{
        model::{
            FakeRuntimeBehavior, FakeRuntimeProvider, ModelManager, ModelManagerConfig,
            ModelRepository,
        },
        model_management::{AvaiModelManagementRpc, ModelManagementConfig, serve_uds},
        model_runtime_tests::policy,
        observability::Observability,
        source::SourcePolicy,
        task::{TaskManager, TaskManagerConfig},
    };
    use base::{
        tokio_util::sync::CancellationToken,
        utils::rt::{GlobalRuntime, RuntimeType},
    };
    use gmv_nodec::component_management::UnsupportedDrainOwner;
    use gmv_protocol::{
        avai::{
            feedback::v1::avai_feedback_client::AvaiFeedbackClient,
            model_management::v1::{
                GetManagementCapabilitiesRequest,
                avai_model_management_client::AvaiModelManagementClient,
            },
        },
        component_management::v1::{
            ComponentProbeRequest, component_management_client::ComponentManagementClient,
        },
    };
    use std::sync::Arc;
    use tonic::transport::Endpoint;

    let fixture = fixture().await;
    let model_repository =
        ModelRepository::open(&fixture.root.join("model.db"), &fixture.root.join("models"))
            .await
            .unwrap();
    let model_manager = ModelManager::open(
        model_repository.clone(),
        vec![Arc::new(FakeRuntimeProvider::new(
            "fake",
            FakeRuntimeBehavior::default(),
        ))],
        ModelManagerConfig::default(),
    )
    .await
    .unwrap();
    let runtime = GlobalRuntime::register_default(RuntimeType::Custom(format!(
        "avai-feedback-management-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
    .unwrap();
    let tasks = TaskManager::open_with_feedback(
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
        vec!["image.metadata.inspect".into()],
        TaskManagerConfig {
            database_path: fixture.root.join("management-task.db"),
            queue_size: 4,
            worker_count: 1,
            source_policy: SourcePolicy::default(),
            max_result_bytes: 1024,
        },
        Some(model_manager.clone()),
        &runtime,
        Arc::new(Observability::new()),
        Some((
            FeedbackConfig {
                enabled: true,
                spool_root: fixture.root.join("management-spool"),
                ..Default::default()
            },
            "installation".into(),
            "host".into(),
        )),
    )
    .await
    .unwrap();
    let model_rpc = AvaiModelManagementRpc::new_with_observability(
        model_repository,
        model_manager,
        tasks.clone(),
        ModelManagementConfig {
            trusted_import_root: fixture.root.join("import"),
            package_policy: policy(),
            receipt_capacity: 16,
            receipt_retention_ms: 86_400_000,
            mutation_concurrency: 1,
        },
        CancellationToken::new(),
        Arc::new(Observability::new()),
    )
    .unwrap();
    let socket = fixture.root.join("management.sock");
    let cancel = CancellationToken::new();
    let server_socket = socket.clone();
    let shutdown = cancel.clone();
    let server = base::tokio::spawn(async move {
        serve_uds(
            &server_socket,
            Arc::new(UnsupportedDrainOwner::new("avai")),
            model_rpc,
            shutdown,
        )
        .await
        .unwrap();
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        base::tokio::task::yield_now().await;
    }
    let channel = Endpoint::try_from(format!("unix://{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut feedback = AvaiFeedbackClient::new(channel.clone());
    assert!(
        feedback
            .get_feedback_capabilities(rpc::GetFeedbackCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner()
            .enabled
    );
    let mut model = AvaiModelManagementClient::new(channel.clone());
    assert!(
        model
            .get_management_capabilities(GetManagementCapabilitiesRequest {})
            .await
            .is_ok()
    );
    let mut component = ComponentManagementClient::new(channel);
    assert!(
        component
            .probe(ComponentProbeRequest {
                operation_id: "probe".into(),
                component_id: "avai".into(),
                readiness_contract_version: 1,
                deadline_epoch_ms: 1_900_000_000_000
            })
            .await
            .is_ok()
    );
    cancel.cancel();
    server.await.unwrap();
    tasks.close_and_wait().await.unwrap();
}
#[tokio::test]
async fn ack_commit_before_file_delete_replays_after_orphan_cleanup() {
    let fixture = fixture().await;
    let id = fixture
        .manager
        .prepare(material(b"evidence"))
        .await
        .unwrap()
        .unwrap();
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    fixture.manager.promote(&id).await.unwrap();
    let package = list(&fixture.manager).await.packages.remove(0);
    let path = fixture
        .config
        .spool_root
        .join(format!("{}_{}.evidence", id, package.evidence_sha256));
    assert!(path.exists());
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO avai_feedback_ack_receipt(feedback_id,content_hash,terminal_outcome,acked_at_ms,expires_at_ms) VALUES(?,?,?,?,?)")
        .bind(&id).bind(&package.content_hash).bind(rpc::FeedbackTerminalOutcome::Accepted as i32)
        .bind(1_800_000_000_000i64).bind(1_900_000_000_000i64)
        .execute(&mut *transaction).await.unwrap();
    sqlx::query("DELETE FROM avai_feedback_outbox WHERE feedback_id=?")
        .bind(&id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let reopened = FeedbackManager::open(
        &fixture.database,
        fixture.config.clone(),
        "installation".into(),
        "host".into(),
        NodeIdentity {
            node_id: "node".into(),
            instance_id: "instance".into(),
            kind: 4,
        },
    )
    .await
    .unwrap();
    assert!(!path.exists());
    assert!(
        rpc(&reopened)
            .ack_feedback(Request::new(rpc::AckFeedbackRequest {
                feedback_id: id,
                content_hash: package.content_hash,
                terminal_outcome: rpc::FeedbackTerminalOutcome::Accepted as i32,
            }))
            .await
            .unwrap()
            .into_inner()
            .replayed
    );
}

#[tokio::test]
async fn missing_or_expired_evidence_is_never_returned() {
    let fixture = fixture().await;
    let pool = base_db::dbx::sqlitex::build_sqlite_pool(
        SqliteConnectionConfig::new(&fixture.database),
        DatabasePoolConfig::default(),
    )
    .unwrap();
    sqlx::query("UPDATE avai_task SET state=3 WHERE task_id='task-1'")
        .execute(&pool)
        .await
        .unwrap();
    let id = fixture
        .manager
        .prepare(material(b"evidence"))
        .await
        .unwrap()
        .unwrap();
    fixture.manager.promote(&id).await.unwrap();
    let package = list(&fixture.manager).await.packages.remove(0);
    let request = rpc::ReadFeedbackEvidenceRequest {
        feedback_id: id.clone(),
        content_hash: package.content_hash,
        evidence_sha256: package.evidence_sha256.clone(),
        expected_size: package.evidence_size_bytes,
    };
    let path = fixture
        .config
        .spool_root
        .join(format!("{}_{}.evidence", id, package.evidence_sha256));
    std::fs::remove_file(&path).unwrap();
    assert!(
        rpc(&fixture.manager)
            .read_feedback_evidence(Request::new(request.clone()))
            .await
            .is_err()
    );
    assert_eq!(list(&fixture.manager).await.packages.len(), 1);
    std::fs::write(&path, b"evidence").unwrap();
    sqlx::query("UPDATE avai_feedback_outbox SET expires_at_ms=1 WHERE feedback_id=?")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(list(&fixture.manager).await.packages.is_empty());
    assert!(
        rpc(&fixture.manager)
            .read_feedback_evidence(Request::new(request))
            .await
            .is_err()
    );
    fixture.manager.sweep().await.unwrap();
    assert!(!path.exists());
}

#[test]
fn invalid_feedback_bounds_are_rejected() {
    let defaults = FeedbackConfig::default();
    assert!(
        FeedbackConfig {
            sample_permyriad: 10_001,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            max_pending_items: 0,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            max_total_bytes: 0,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            ttl_ms: 0,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            evidence_chunk_bytes: 0,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            ack_receipt_capacity: 0,
            ..defaults.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        FeedbackConfig {
            ack_receipt_retention_ms: 0,
            ..defaults
        }
        .validate()
        .is_err()
    );
}
#[tokio::test]
async fn byte_bound_and_source_failure_leave_task_truth_authoritative() {
    use crate::{
        observability::Observability,
        source::SourcePolicy,
        task::{TaskManager, TaskManagerConfig},
    };
    use base::utils::rt::{GlobalRuntime, RuntimeType};
    use gmv_protocol::{
        avai::v1::{AiTaskState, CreateTaskRequest, QueryTaskRequest},
        common::v1::OperationRef,
    };
    use std::{
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "avai-feedback-byte-limit-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = GlobalRuntime::register_default(RuntimeType::Custom(format!(
        "avai-feedback-byte-limit-{id}"
    )))
    .unwrap();
    let identity = NodeIdentity {
        node_id: "node".into(),
        instance_id: "instance".into(),
        kind: 4,
    };
    let manager = TaskManager::open_with_feedback(
        identity.clone(),
        vec!["image.metadata.inspect".into()],
        TaskManagerConfig {
            database_path: root.join("tasks.db"),
            queue_size: 4,
            worker_count: 1,
            source_policy: SourcePolicy {
                allow_private_image_urls: true,
                ..Default::default()
            },
            max_result_bytes: 1024,
        },
        None,
        &runtime,
        Arc::new(Observability::new()),
        Some((
            FeedbackConfig {
                enabled: true,
                spool_root: root.join("spool"),
                capabilities: HashSet::from(["image.metadata.inspect".into()]),
                sample_permyriad: 10_000,
                max_total_bytes: 1,
                ..Default::default()
            },
            "installation".into(),
            "host".into(),
        )),
    )
    .await
    .unwrap();
    let png = base::base64::Engine::decode(&base::base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    for (task_id, bytes, expected) in [
        ("byte-full", png, AiTaskState::Succeeded),
        (
            "source-failure",
            b"not a valid PNG".to_vec(),
            AiTaskState::Failed,
        ),
    ] {
        let (url, server) = image_server(bytes).await;
        let request = CreateTaskRequest {
            operation: Some(OperationRef {
                operation_id: format!("op-{task_id}"),
                idempotency_key: format!("idem-{task_id}"),
            }),
            task_id: task_id.into(),
            capability: "image.metadata.inspect".into(),
            expected_avai: Some(identity.clone()),
            source: Some(SourceSpec {
                source: Some(Source::ImageUrl(ImageUrlSource {
                    url,
                    expected: None,
                    max_bytes: 1024,
                })),
            }),
            ..Default::default()
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        manager.create_task(request, now).await;
        let state = base::tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = manager
                    .query_task(QueryTaskRequest {
                        task_id: task_id.into(),
                    })
                    .await
                    .state;
                if state != AiTaskState::Pending as i32 && state != AiTaskState::Running as i32 {
                    break state;
                }
                base::tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(state, expected as i32);
        server.await.unwrap();
    }
    for (task_id, deadline) in [("cancelled", false), ("deadline", true)] {
        use base::tokio::io::AsyncReadExt;
        let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (entered_tx, entered_rx) = base::tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = base::tokio::sync::oneshot::channel::<()>();
        let server = base::tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = connection.read(&mut request).await.unwrap();
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        });
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let request = CreateTaskRequest {
            operation: Some(OperationRef {
                operation_id: format!("op-{task_id}"),
                idempotency_key: format!("idem-{task_id}"),
            }),
            task_id: task_id.into(),
            capability: "image.metadata.inspect".into(),
            expected_avai: Some(identity.clone()),
            deadline_epoch_ms: if deadline { now + 250 } else { now + 5_000 },
            source: Some(SourceSpec {
                source: Some(Source::ImageUrl(ImageUrlSource {
                    url: format!("http://{address}/image.png"),
                    expected: None,
                    max_bytes: 1024,
                })),
            }),
            ..Default::default()
        };
        assert_eq!(
            manager.create_task(request, now).await.state,
            AiTaskState::Pending as i32
        );
        base::tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        if !deadline {
            let cancelled = manager
                .cancel_task(gmv_protocol::avai::v1::CancelTaskRequest {
                    task_id: task_id.into(),
                    ..Default::default()
                })
                .await;
            assert_eq!(cancelled.state, AiTaskState::Cancelled as i32);
        }
        let state = base::tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = manager
                    .query_task(QueryTaskRequest {
                        task_id: task_id.into(),
                    })
                    .await
                    .state;
                if state != AiTaskState::Pending as i32 && state != AiTaskState::Running as i32 {
                    break state;
                }
                base::tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            state,
            (if deadline {
                AiTaskState::Failed
            } else {
                AiTaskState::Cancelled
            }) as i32
        );
        release_tx.send(()).unwrap();
        server.await.unwrap();
    }
    assert!(
        list(&manager.feedback_manager().unwrap())
            .await
            .packages
            .is_empty()
    );
    manager.close_and_wait().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
