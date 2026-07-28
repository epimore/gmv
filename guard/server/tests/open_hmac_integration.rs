use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::api::v2::ApiV2;
use gmv_guard_server::api::v2::http::{HttpState, router};
use gmv_guard_server::auth::{AuthState, SessionPolicy};
use gmv_guard_server::integration::hmac::{
    HmacNonceCache, SignedRequest, body_sha256, sign_request,
};
use gmv_guard_server::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationCredential, IntegrationTransport,
};
use gmv_guard_server::integration::secret::IntegrationSecretCipher;
use gmv_guard_server::operation::OperationService;
use gmv_guard_server::outbox::OutboxRepository;
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::persistent::IntegrationRepository;
use gmv_guard_server::store::sqlite::SqliteStore;
use tower::ServiceExt;

#[test]
fn open_api_accepts_valid_hmac_and_rejects_nonce_replay() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path =
                std::env::temp_dir().join(format!("guard-open-hmac-{}.db", uuid::Uuid::new_v4()));
            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            let store = SqliteStore::new(pool);
            store.migrate().await.unwrap();
            let integrations = IntegrationRepository::Sqlite(store);
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            integrations
                .upsert(&Integration {
                    integration_id: "integration-test".to_string(),
                    name: "HMAC test".to_string(),
                    transport: IntegrationTransport::Http,
                    inbound_enabled: true,
                    outbound_enabled: false,
                    enabled: true,
                    scopes: vec!["nodes:read".to_string()],
                    expires_at_ms: None,
                    config_version: 1,
                    created_by: "test".to_string(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                })
                .await
                .unwrap();
            let cipher = IntegrationSecretCipher::from_base64_key_no_pad(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
            .unwrap();
            let secret = "test-integration-secret";
            integrations
                .insert_credential(&IntegrationCredential {
                    credential_id: "credential-test".to_string(),
                    access_key: "ak_test".to_string(),
                    integration_id: "integration-test".to_string(),
                    purpose: CredentialPurpose::HttpInboundVerify,
                    secret_ciphertext: cipher.encrypt(secret).unwrap(),
                    key_version: 1,
                    status: CredentialStatus::Active,
                    not_before_ms: now_ms - 1,
                    expires_at_ms: None,
                    revoked_at_ms: None,
                    created_by: "test".to_string(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                })
                .await
                .unwrap();
            let memory = InMemoryGuardStore::default();
            let app = router(HttpState {
                api: ApiV2::new(memory.clone(), OperationService::default()),
                auth: AuthState::new(
                    [],
                    SessionPolicy {
                        allowed_origins: vec!["http://127.0.0.1".to_string()],
                        secure_cookie: false,
                        session_ttl: Duration::from_secs(3_600),
                        login_window: Duration::from_secs(60),
                        max_failed_attempts: 5,
                        local_admin_username: None,
                        local_admin_login_only: false,
                    },
                ),
                outbox: OutboxRepository::from(memory),
                users: None,
                integrations: Some(integrations),
                integration_secrets: Some(cipher),
                integration_nonces: HmacNonceCache::new(300_000, 100).unwrap(),
                mqtt_runtime_protocol_version: "v3".to_string(),
                mqtt_runtime_enabled: false,
                event_forwarder: None,
                media_https_http2_verified: false,
            });
            let uri = "/openapi/v1/nodes?zone=edge";
            let nonce = "0123456789abcdef";
            let signature = sign_request(
                secret.as_bytes(),
                &SignedRequest {
                    access_key: "ak_test",
                    timestamp_ms: now_ms,
                    nonce,
                    method: "GET",
                    path: "/openapi/v1/nodes",
                    query: "zone=edge",
                    body: b"",
                },
            )
            .unwrap();
            let signed_request = || {
                Request::get(uri)
                    .header("x-gmv-access-key", "ak_test")
                    .header("x-gmv-timestamp", now_ms.to_string())
                    .header("x-gmv-nonce", nonce)
                    .header("x-gmv-content-sha256", body_sha256(b""))
                    .header("x-gmv-signature", &signature)
                    .body(Body::empty())
                    .unwrap()
            };
            assert_eq!(
                app.clone()
                    .oneshot(signed_request())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
            assert_eq!(
                app.oneshot(signed_request()).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );

            let _ = std::fs::remove_file(path);
        });
}
