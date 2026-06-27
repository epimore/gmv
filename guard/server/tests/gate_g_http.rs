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
use guard::sim::{EndpointMode, Simulator};
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

fn app(simulator_enabled: bool) -> (axum::Router, InMemoryGuardStore) {
    let store = InMemoryGuardStore::default();
    let simulator = simulator_enabled.then(|| {
        let simulator = Simulator::new(store.clone(), EndpointMode::Multi);
        simulator.bootstrap(1_000).unwrap();
        simulator
    });
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
            simulator,
            users: None,
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
fn ui_api_completes_simulated_stream_ptz_ai_and_stop_loop() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, _) = app(true);
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
            let device_id = devices[0]["device_id"].as_str().unwrap();

            let (status, _, stream) = call(
                &app,
                write_request(
                    &format!("/api/v2/devices/{device_id}/preview"),
                    &cookie,
                    &csrf,
                    json!({ "request_id": "ui-1", "channel_id": "ch-1" }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            assert_eq!(stream["state"], "running");
            assert!(stream["endpoint"].as_str().unwrap().contains(','));
            let stream_id = stream["stream_id"].as_str().unwrap();

            let (status, _, ptz) = call(
                &app,
                write_request(
                    &format!("/api/v2/devices/{device_id}/ptz"),
                    &cookie,
                    &csrf,
                    json!({ "channel_id": "ch-1" }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(ptz["count"], 1);

            let (status, _, task) = call(
                &app,
                write_request(
                    "/api/v2/ai/tasks",
                    &cookie,
                    &csrf,
                    json!({ "request_id": "ui-ai-1", "stream_id": stream_id, "model": "vehicle" }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            let task_id = task["task_id"].as_str().unwrap();
            assert_eq!(task["state"], "running");

            assert_eq!(
                call(
                    &app,
                    write_request(
                        &format!("/api/v2/ai/tasks/{task_id}/cancel"),
                        &cookie,
                        &csrf,
                        json!({}),
                    ),
                )
                .await
                .0,
                StatusCode::OK
            );
            assert_eq!(
                call(
                    &app,
                    write_request(
                        &format!("/api/v2/streams/{stream_id}/stop"),
                        &cookie,
                        &csrf,
                        json!({}),
                    ),
                )
                .await
                .0,
                StatusCode::OK
            );
        });
}

#[test]
fn guard_interruption_and_outbox_manual_retry_are_exposed_safely() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, store) = app(true);
            let (operator_cookie, operator_csrf) = login(&app, "operator").await;
            let stream = call(
                &app,
                write_request(
                    "/api/v2/devices/34020000001320000001/preview",
                    &operator_cookie,
                    &operator_csrf,
                    json!({ "request_id": "keep", "channel_id": "ch-1" }),
                ),
            )
            .await
            .2;
            let stream_id = stream["stream_id"].as_str().unwrap();

            let (admin_cookie, admin_csrf) = login(&app, "admin").await;
            assert_eq!(
                call(
                    &app,
                    write_request(
                        "/api/v2/sim/availability",
                        &admin_cookie,
                        &admin_csrf,
                        json!({ "available": false }),
                    ),
                )
                .await
                .0,
                StatusCode::OK
            );
            assert_eq!(
                call(
                    &app,
                    write_request(
                        "/api/v2/devices/34020000001320000001/preview",
                        &operator_cookie,
                        &operator_csrf,
                        json!({ "request_id": "blocked", "channel_id": "ch-2" }),
                    ),
                )
                .await
                .0,
                StatusCode::CONFLICT
            );
            let status = call(
                &app,
                Request::get("/api/v2/sim/status")
                    .header(COOKIE, &operator_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .2;
            assert_eq!(status["running_streams"], 1);
            assert_eq!(
                call(
                    &app,
                    write_request(
                        "/api/v2/sim/availability",
                        &admin_cookie,
                        &admin_csrf,
                        json!({ "available": true }),
                    ),
                )
                .await
                .0,
                StatusCode::OK
            );
            let streams = call(
                &app,
                Request::get("/api/v2/streams")
                    .header(COOKIE, &operator_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .2;
            assert_eq!(streams[0]["stream_id"], stream_id);

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
fn simulator_disabled_is_explicit() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let (app, _) = app(false);
            let (cookie, _) = login(&app, "viewer").await;
            let response = call(
                &app,
                Request::get("/api/v2/devices")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(response.0, StatusCode::NOT_IMPLEMENTED);
            assert_eq!(response.2["code"], "simulator_disabled");
        });
}
