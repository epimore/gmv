use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString};
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base::serde_json::{Value, json};
use gmv_guard_server::api::v2::ApiV2;
use gmv_guard_server::api::v2::http::{HttpState, router};
use gmv_guard_server::auth::{AuthState, Role, SessionPolicy, UserAccount};
use gmv_guard_server::operation::OperationService;
use gmv_guard_server::outbox::OutboxRepository;
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::model::{
    EventRecord, OutboxDestinationKind, OutboxRecord, OutboxState,
};
use tower::ServiceExt;

const ORIGIN_VALUE: &str = "http://127.0.0.1:5173";

fn password_hash() -> String {
    let salt = SaltString::encode_b64(b"gmv-gate-r-tests").unwrap();
    Argon2::default()
        .hash_password(b"secret", &salt)
        .unwrap()
        .to_string()
}

fn app() -> (axum::Router, InMemoryGuardStore) {
    let store = InMemoryGuardStore::default();
    let hash = password_hash();
    let auth = AuthState::new(
        [
            UserAccount::new("viewer", Role::Viewer, hash.clone()),
            UserAccount::new("operator", Role::Operator, hash.clone()),
            UserAccount::new("admin", Role::Admin, hash),
        ],
        SessionPolicy {
            allowed_origins: vec![ORIGIN_VALUE.to_string()],
            secure_cookie: false,
            session_ttl: Duration::from_secs(3600),
            login_window: Duration::from_secs(60),
            max_failed_attempts: 3,
            local_admin_username: None,
            local_admin_login_only: false,
        },
    );
    (
        router(HttpState {
            api: ApiV2::new(store.clone(), OperationService::default()),
            auth,
            outbox: OutboxRepository::from(store.clone()),
            users: None,
            integrations: None,
            commands: None,
            integration_secrets: None,
            integration_nonces: gmv_guard_server::integration::hmac::HmacNonceCache::new(
                300_000, 100,
            )
            .unwrap(),
            mqtt_runtime_protocol_version: "v3".to_string(),
            mqtt_runtime_enabled: false,
            event_forwarder: None,
            media_https_http2_verified: false,
        }),
        store,
    )
}

async fn call_json(
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

async fn call_text(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn login(app: &axum::Router, username: &str) -> (String, String) {
    let (status, headers, body) = call_json(
        app,
        Request::post("/api/v2/auth/login")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "username": username, "password": "secret" }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (
        headers
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string(),
        body["csrf_token"].as_str().unwrap().to_string(),
    )
}

fn write_request(path: &str, cookie: &str, csrf: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header(ORIGIN, ORIGIN_VALUE)
        .header(COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[test]
fn gate_r_real_device_readiness_and_observability_contract() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, store) = app();
            let (status, _, live) = call_json(
                &app,
                Request::get("/health/live").body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(live["status"], "live");
            let (status, _, ready) = call_json(
                &app,
                Request::get("/health/ready").body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(ready["status"], "ready");

            let (operator_cookie, operator_csrf) = login(&app, "operator").await;
            let stream = call_json(
                &app,
                write_request(
                    "/api/v2/devices/34020000001320000001/preview",
                    &operator_cookie,
                    &operator_csrf,
                    json!({ "request_id": "gate-r-live", "channel_id": "ch-1" }),
                ),
            )
            .await;
            assert_eq!(stream.0, StatusCode::NOT_FOUND);
            assert_eq!(stream.2["code"], "node_not_found");
            assert_eq!(
                stream.2["user_message"],
                "未找到可用节点，请确认服务已启动并注册到 Guard"
            );

            let ptz = call_json(
                &app,
                write_request(
                    "/api/v2/devices/34020000001320000001/ptz",
                    &operator_cookie,
                    &operator_csrf,
                    json!({
                        "channel_id": "ch-1",
                        "leftRight": 1,
                        "upDown": 0,
                        "inOut": 0,
                        "horizonSpeed": 64,
                        "verticalSpeed": 0,
                        "zoomSpeed": 0
                    }),
                ),
            )
            .await;
            assert_eq!(ptz.0, StatusCode::NOT_FOUND);
            assert_eq!(ptz.2["code"], "node_not_found");
            assert_eq!(
                ptz.2["user_message"],
                "未找到可用节点，请确认服务已启动并注册到 Guard"
            );

            let status = call_json(
                &app,
                Request::get("/api/v2/runtime/status")
                    .header(COOKIE, &operator_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .2;
            assert_eq!(status["running_streams"], 0);
            assert_eq!(status["running_ai_tasks"], 0);

            store
                .insert_event_with_outbox(
                    EventRecord {
                        event_id: "gate-r-event".to_string(),
                        topic: "gate.r.webhook".to_string(),
                        priority: 1,
                        payload: b"{}".to_vec(),
                    },
                    vec![
                        OutboxRecord {
                            outbox_id: "gate-r-pending".to_string(),
                            event_id: "gate-r-event".to_string(),
                            integration_id: String::new(),
                            mapping_id: String::new(),
                            destination_kind: OutboxDestinationKind::Webhook,
                            destination: "https://example.com/hook".to_string(),
                            payload: b"{}".to_vec(),
                            state: OutboxState::Pending,
                            attempts: 0,
                            next_attempt_at_ms: 0,
                            last_error: None,
                            created_at_ms: 1,
                            updated_at_ms: 1,
                            expires_at_ms: None,
                        },
                        OutboxRecord {
                            outbox_id: "gate-r-dead".to_string(),
                            event_id: "gate-r-event".to_string(),
                            integration_id: String::new(),
                            mapping_id: String::new(),
                            destination_kind: OutboxDestinationKind::Mqtt,
                            destination: "gmv/events".to_string(),
                            payload: b"{}".to_vec(),
                            state: OutboxState::Dead,
                            attempts: 8,
                            next_attempt_at_ms: 0,
                            last_error: Some("broker offline".to_string()),
                            created_at_ms: 2,
                            updated_at_ms: 2,
                            expires_at_ms: None,
                        },
                    ],
                )
                .unwrap();
            let (status, metrics) =
                call_text(&app, Request::get("/metrics").body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::OK);
            assert!(metrics.contains("gmv_guard_nodes 0"));
            assert!(metrics.contains("gmv_guard_outbox_backlog 1"));
            assert!(metrics.contains("gmv_guard_outbox_dead 1"));
        });
}

#[test]
fn gate_r_login_rate_limit_error_does_not_echo_secret() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, _) = app();
            let mut last = (StatusCode::OK, Value::Null);
            for _ in 0..4 {
                let (status, _, body) = call_json(
                    &app,
                    Request::post("/api/v2/auth/login")
                        .header(ORIGIN, ORIGIN_VALUE)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({ "username": "operator", "password": "wrong-secret" })
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await;
                last = (status, body);
            }
            assert_eq!(last.0, StatusCode::TOO_MANY_REQUESTS);
            let body_text = last.1.to_string();
            assert!(body_text.contains("rate_limited"));
            assert!(!body_text.contains("wrong-secret"));
            assert!(!body_text.contains("secret"));
        });
}
