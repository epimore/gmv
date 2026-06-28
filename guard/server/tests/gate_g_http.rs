use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString};
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base::serde_json::{Value, json};
use guard::api::v2::ApiV2;
use guard::api::v2::http::{HttpState, router};
use guard::auth::{AuthState, Role, SessionPolicy, UserAccount};
use guard::job::SystemJobService;
use guard::operation::OperationService;
use guard::outbox::OutboxRepository;
use guard::store::InMemoryGuardStore;
use guard::store::model::{EventRecord, OutboxDestinationKind, OutboxRecord, OutboxState};
use tower::ServiceExt;

const ORIGIN_VALUE: &str = "http://127.0.0.1:5173";

fn password_hash() -> String {
    let salt = SaltString::encode_b64(b"gmv-gate-g-tests").unwrap();
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
            max_failed_attempts: 5,
            local_admin_username: None,
            local_admin_login_only: false,
        },
    );
    (
        router(HttpState {
            api: ApiV2::new(
                store.clone(),
                OperationService::default(),
                SystemJobService::default(),
            ),
            auth,
            outbox: OutboxRepository::from(store.clone()),
            users: None,
            media: Default::default(),
            media_files: None,
            gb28181: None,
            event_forwarder: None,
        }),
        store,
    )
}

async fn call(
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

async fn login(app: &axum::Router, username: &str) -> (String, String) {
    let (status, headers, body) = call(
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
fn ui_api_requires_registered_nodes_for_device_operations() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, _) = app();
            let (cookie, csrf) = login(&app, "operator").await;
            let (status, _, devices) = call(
                &app,
                Request::get("/api/v2/devices")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(devices.as_array().unwrap().len(), 0);

            let (status, _, _) = call(
                &app,
                write_request(
                    "/api/v2/devices/34020000001320000001/preview",
                    &cookie,
                    &csrf,
                    json!({ "request_id": "ui-1", "channel_id": "ch-1" }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);

            let (status, _, _) = call(
                &app,
                write_request(
                    "/api/v2/devices/34020000001320000001/ptz",
                    &cookie,
                    &csrf,
                    json!({ "channel_id": "ch-1" }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        });
}

#[test]
fn outbox_manual_retry_is_exposed_safely() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, store) = app();
            let (operator_cookie, operator_csrf) = login(&app, "operator").await;

            let status = call(
                &app,
                Request::get("/api/v2/runtime/status")
                    .header(COOKIE, &operator_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .2;
            assert_eq!(status["running_streams"], 0);

            store
                .insert_event_with_outbox(
                    EventRecord {
                        event_id: "event-dead".to_string(),
                        topic: "webhook.failed".to_string(),
                        priority: 2,
                        payload: b"{}".to_vec(),
                    },
                    vec![OutboxRecord {
                        outbox_id: "outbox-dead".to_string(),
                        event_id: "event-dead".to_string(),
                        destination_kind: OutboxDestinationKind::Webhook,
                        destination: "https://example.com/hook".to_string(),
                        payload: b"{}".to_vec(),
                        state: OutboxState::Dead,
                        attempts: 8,
                        next_attempt_at_ms: 0,
                        last_error: Some("offline".to_string()),
                        created_at_ms: 1,
                        updated_at_ms: 1,
                    }],
                )
                .unwrap();
            let list = call(
                &app,
                Request::get("/api/v2/integrations/outbox")
                    .header(COOKIE, &operator_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(list.0, StatusCode::OK);
            assert_eq!(list.2[0]["state"], "dead");
            let retried = call(
                &app,
                write_request(
                    "/api/v2/integrations/outbox/outbox-dead/retry",
                    &operator_cookie,
                    &operator_csrf,
                    json!({}),
                ),
            )
            .await;
            assert_eq!(retried.0, StatusCode::OK);
            assert_eq!(retried.2["state"], "pending");
        });
}

#[test]
fn devices_are_empty_without_registered_device_source() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, _) = app();
            let (cookie, _) = login(&app, "viewer").await;
            let response = call(
                &app,
                Request::get("/api/v2/devices")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(response.0, StatusCode::OK);
            assert_eq!(response.2.as_array().unwrap().len(), 0);
        });
}
