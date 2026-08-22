use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base::serde_json::{Value, json};
use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::api::v2::ApiV2;
use gmv_guard_server::api::v2::http::{HttpState, router};
use gmv_guard_server::auth::{AuthState, Role, SessionPolicy, UserAccount, hash_password};
use gmv_guard_server::integration::hmac::{
    HmacNonceCache, SignedRequest, body_sha256, sign_request,
};
use gmv_guard_server::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationCredential, IntegrationTransport,
};
use gmv_guard_server::integration::secret::{IntegrationSecretCipher, IntegrationSecretManager};
use gmv_guard_server::operation::OperationService;
use gmv_guard_server::outbox::OutboxRepository;
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::persistent::{CommandRepository, IntegrationRepository};
use gmv_guard_server::store::sqlite::SqliteStore;
use tower::ServiceExt;

const UI_ORIGIN: &str = "http://127.0.0.1";

async fn json_response(
    app: &axum::Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        base::serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, body)
}

async fn login_admin(app: &axum::Router) -> (String, String) {
    let (status, headers, body) = json_response(
        app,
        Request::post("/api/v2/auth/login")
            .header(ORIGIN, UI_ORIGIN)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "username": "admin", "password": "admin-password" }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (cookie, body["csrf_token"].as_str().unwrap().to_string())
}

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
            let commands = CommandRepository::Sqlite(store.clone());
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
                    scopes: vec!["*".to_string()],
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
                commands: Some(commands),
                integration_secrets: Some(IntegrationSecretManager::new(cipher)),
                integration_nonces: HmacNonceCache::new(300_000, 100).unwrap(),
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
                    request_id: "",
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
                app.clone()
                    .oneshot(signed_request())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );

            let post_uri = "/openapi/v1/ai/tasks/missing/cancel";
            let request_id = "request-replay-1";
            let signed_post = |nonce: &'static str| {
                let signature = sign_request(
                    secret.as_bytes(),
                    &SignedRequest {
                        access_key: "ak_test",
                        timestamp_ms: now_ms,
                        nonce,
                        method: "POST",
                        path: post_uri,
                        query: "",
                        request_id,
                        body: b"",
                    },
                )
                .unwrap();
                Request::post(post_uri)
                    .header("x-gmv-access-key", "ak_test")
                    .header("x-gmv-timestamp", now_ms.to_string())
                    .header("x-gmv-nonce", nonce)
                    .header("x-gmv-content-sha256", body_sha256(b""))
                    .header("x-gmv-request-id", request_id)
                    .header("x-gmv-signature", signature)
                    .body(Body::empty())
                    .unwrap()
            };
            let first = app
                .clone()
                .oneshot(signed_post("1111111111111111"))
                .await
                .unwrap();
            let first_status = first.status();
            let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
            let replay = app
                .clone()
                .oneshot(signed_post("2222222222222222"))
                .await
                .unwrap();
            let replay_status = replay.status();
            let replay_body = to_bytes(replay.into_body(), usize::MAX).await.unwrap();
            assert_eq!(replay_status, first_status);
            assert_eq!(replay_body, first_body);

            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn credential_secret_reveal_requires_secondary_authentication() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-credential-reveal-{}.db",
                uuid::Uuid::new_v4()
            ));
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
            let commands = CommandRepository::Sqlite(store.clone());
            let integrations = IntegrationRepository::Sqlite(store);
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            integrations
                .upsert(&Integration {
                    integration_id: "integration-reveal".to_string(),
                    name: "Credential reveal".to_string(),
                    transport: IntegrationTransport::Http,
                    inbound_enabled: true,
                    outbound_enabled: false,
                    enabled: true,
                    scopes: vec!["*".to_string()],
                    expires_at_ms: None,
                    config_version: 1,
                    created_by: "admin".to_string(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                })
                .await
                .unwrap();
            let cipher = IntegrationSecretCipher::from_base64_key_no_pad(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
            .unwrap();
            let secret = "persistent-http-hmac-secret";
            integrations
                .insert_credential(&IntegrationCredential {
                    credential_id: "credential-reveal".to_string(),
                    access_key: "ak_reveal".to_string(),
                    integration_id: "integration-reveal".to_string(),
                    purpose: CredentialPurpose::HttpInboundVerify,
                    secret_ciphertext: cipher.encrypt(secret).unwrap(),
                    key_version: 1,
                    status: CredentialStatus::Active,
                    not_before_ms: now_ms - 1,
                    expires_at_ms: None,
                    revoked_at_ms: None,
                    created_by: "admin".to_string(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                })
                .await
                .unwrap();
            let memory = InMemoryGuardStore::default();
            let app = router(HttpState {
                api: ApiV2::new(memory.clone(), OperationService::default()),
                auth: AuthState::new(
                    [UserAccount::new(
                        "admin",
                        Role::Admin,
                        hash_password("admin-password").unwrap(),
                    )],
                    SessionPolicy {
                        allowed_origins: vec![UI_ORIGIN.to_string()],
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
                commands: Some(commands),
                integration_secrets: Some(IntegrationSecretManager::new(cipher)),
                integration_nonces: HmacNonceCache::new(300_000, 100).unwrap(),
                event_forwarder: None,
                media_https_http2_verified: false,
            });
            let (cookie, csrf) = login_admin(&app).await;

            let (status, _, list) = json_response(
                &app,
                Request::get("/api/v2/integrations/integration-reveal/credentials")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(list[0]["access_key"], "ak_reveal");
            assert!(list[0].get("secret").is_none());

            let reveal = |password: &str| {
                Request::post(
                    "/api/v2/integrations/integration-reveal/credentials/credential-reveal/reveal",
                )
                .header(ORIGIN, UI_ORIGIN)
                .header(COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "password": password }).to_string()))
                .unwrap()
            };
            let (status, _, body) = json_response(&app, reveal("wrong-password")).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["code"], "secondary_auth_failed");
            assert_eq!(body["user_message"], "当前密码验证失败");

            let (status, _, body) = json_response(&app, reveal("admin-password")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["secret"], secret);

            let (status, _, _) = json_response(
                &app,
                Request::post(
                    "/api/v2/integrations/integration-reveal/credentials/credential-reveal/revoke",
                )
                .header(ORIGIN, UI_ORIGIN)
                .header(COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);

            let (status, _, body) = json_response(&app, reveal("admin-password")).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(body["code"], "conflict");

            let _ = std::fs::remove_file(path);
        });
}
