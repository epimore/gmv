use super::*;
use gmv_protocol::{
    avai::v1::{FinalizeImageUploadRequest, PrepareImageUploadRequest},
    common::v1::{NodeKind, OperationRef},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

fn identity() -> NodeIdentity {
    NodeIdentity {
        node_id: "avai-upload-test".to_string(),
        instance_id: "instance-upload-test".to_string(),
        kind: NodeKind::Avai as i32,
    }
}

fn png_bytes() -> Vec<u8> {
    use base::base64::Engine;
    base::base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap()
}

#[tokio::test]
async fn upload_is_idempotent_persistent_and_finalizes_to_local_source() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("avai-upload-test-{}-{id}", std::process::id()));
    let config = UploadManagerConfig {
        database_path: root.join("avai.db"),
        object_root: root.join("objects"),
        public_url: "http://127.0.0.1:19081".to_string(),
        max_image_bytes: 1024,
    };
    let manager = UploadManager::open(
        identity(),
        vec!["image.metadata.inspect".to_string()],
        config.clone(),
    )
    .await
    .unwrap();
    let request = PrepareImageUploadRequest {
        operation: Some(OperationRef {
            operation_id: "prepare-1".to_string(),
            idempotency_key: "prepare-1".to_string(),
        }),
        expected_avai: Some(identity()),
        capability: "image.metadata.inspect".to_string(),
        content_type: "image/png".to_string(),
        max_bytes: 1024,
        deadline_epoch_ms: now_epoch_ms() + 60_000,
    };
    let (first, repeated) = base::tokio::join!(
        manager.prepare(request.clone(), now_epoch_ms()),
        manager.prepare(request, now_epoch_ms())
    );
    assert_eq!(
        first.ticket.as_ref().unwrap().upload_id,
        repeated.ticket.as_ref().unwrap().upload_id
    );
    let ticket = first.ticket.unwrap();
    let proof = URL_SAFE_NO_PAD.encode(&ticket.proof);
    let bytes = png_bytes();
    assert_eq!(
        manager
            .accept(
                &ticket.upload_id,
                "wrong",
                "image/png",
                &bytes,
                now_epoch_ms()
            )
            .await
            .unwrap_err()
            .code,
        "upload_denied"
    );
    let response = routes(manager.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/internal/uploads/{}", ticket.upload_id))
                .header("x-gmv-upload-proof", &proof)
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(bytes.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let finalize = FinalizeImageUploadRequest {
        operation: Some(OperationRef {
            operation_id: "finalize-1".to_string(),
            idempotency_key: "finalize-1".to_string(),
        }),
        expected_avai: Some(identity()),
        upload_id: ticket.upload_id.clone(),
        capability: "image.metadata.inspect".to_string(),
    };
    let source = manager.finalize(finalize.clone(), now_epoch_ms()).await;
    assert!(source.error.is_none());
    assert_eq!(
        source.source.unwrap().metadata.unwrap().sha256,
        format!("{:x}", Sha256::digest(&bytes))
    );
    manager.close().await;

    let reopened = UploadManager::open(
        identity(),
        vec!["image.metadata.inspect".to_string()],
        config,
    )
    .await
    .unwrap();
    assert!(
        reopened
            .finalize(finalize, now_epoch_ms())
            .await
            .error
            .is_none()
    );
    assert_eq!(
        reopened
            .cleanup_expired(ticket.expires_at_epoch_ms + 1)
            .await
            .unwrap(),
        1
    );
    assert!(!root.join("objects").join(&ticket.upload_id).exists());
    reopened.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalid_public_url_is_rejected_before_startup() {
    let root = std::env::temp_dir().join(format!(
        "avai-invalid-upload-config-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let error = UploadManager::open(
        identity(),
        vec!["image.metadata.inspect".to_string()],
        UploadManagerConfig {
            database_path: root.join("avai.db"),
            object_root: root.join("objects"),
            public_url: "http://user:password@127.0.0.1:19081?token=secret".to_string(),
            max_image_bytes: 1024,
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "invalid_upload_config");
    assert!(!root.exists());
}
