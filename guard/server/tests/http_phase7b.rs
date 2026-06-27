use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString};
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base::serde_json::{Value, json};
use guard::api::v2::ApiV2;
use guard::api::v2::http::{HttpState, router};
use guard::auth::{AuthState, Role, SessionPolicy, UserAccount};
use guard::core::LeaseState;
use guard::job::SystemJobService;
use guard::operation::OperationService;
use guard::outbox::OutboxRepository;
use guard::store::InMemoryGuardStore;
use guard::store::model::{EventRecord, LeaseRecord};
use tower::ServiceExt;

const ORIGIN_VALUE: &str = "http://127.0.0.1:5173";

fn run_async(future: impl std::future::Future<Output = ()>) {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(future);
}

fn password_hash(password: &str) -> String {
    let salt = SaltString::encode_b64(b"gmv-guard-tests1").unwrap();
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

fn test_app(store: InMemoryGuardStore) -> axum::Router {
    let hash = password_hash("secret");
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
    router(HttpState {
        api: ApiV2::new(
            store.clone(),
            OperationService::default(),
            SystemJobService::default(),
        ),
        auth,
        outbox: OutboxRepository::from(store),
        users: None,
        media: Default::default(),
        media_files: None,
        event_forwarder: None,
    })
}

async fn request(
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
    let (status, headers, body) = request(
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
    let cookie = headers
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    (cookie, csrf)
}

#[test]
fn session_security_headers_csrf_and_operation_rbac() {
    run_async(async {
        let app = test_app(InMemoryGuardStore::default());
        let (status, headers, _) = request(
            &app,
            Request::post("/api/v2/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "username": "operator", "password": "secret" }).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(headers.contains_key(CONTENT_SECURITY_POLICY));

        let (cookie, csrf) = login(&app, "operator").await;
        let (status, headers, body) = request(
            &app,
            Request::get("/api/v2/auth/session")
                .header(COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["role"], "operator");
        assert!(headers.contains_key(CONTENT_SECURITY_POLICY));

        let operation = json!({
            "operation_id": "op-1",
            "kind": "node.takeover",
            "dangerous": true,
            "confirmation": "node.takeover"
        });
        let (status, _, _) = request(
            &app,
            Request::post("/api/v2/operations")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &cookie)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(operation.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _, body) = request(
            &app,
            Request::post("/api/v2/operations")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(operation.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["status"], "accepted");

        let (status, _, _) = request(
            &app,
            Request::post("/api/v2/auth/logout")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    });
}

#[test]
fn viewer_cannot_start_operation_and_only_admin_starts_system_job() {
    run_async(async {
        let app = test_app(InMemoryGuardStore::default());
        let (viewer_cookie, viewer_csrf) = login(&app, "viewer").await;
        let (status, _, _) = request(
            &app,
            Request::post("/api/v2/operations")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, viewer_cookie)
                .header("x-csrf-token", viewer_csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "operation_id": "op-viewer",
                        "kind": "stream.stop",
                        "dangerous": false,
                        "confirmation": null
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (operator_cookie, operator_csrf) = login(&app, "operator").await;
        let (status, _, _) = request(
            &app,
            Request::post("/api/v2/operations")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &operator_cookie)
                .header("x-csrf-token", &operator_csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "operation_id": "op-operator",
                        "kind": "scheduler.rebalance",
                        "dangerous": false,
                        "confirmation": null
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let job_body = json!({ "job_id": "job-1", "job_type": "backup" }).to_string();
        let (status, _, _) = request(
            &app,
            Request::post("/api/v2/system/jobs")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &operator_cookie)
                .header("x-csrf-token", &operator_csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(job_body.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (admin_cookie, admin_csrf) = login(&app, "admin").await;
        let (status, _, body) = request(
            &app,
            Request::post("/api/v2/system/jobs")
                .header(ORIGIN, ORIGIN_VALUE)
                .header(COOKIE, &admin_cookie)
                .header("x-csrf-token", &admin_csrf)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(job_body))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["job_type"], "backup");

        let (status, _, operations) = request(
            &app,
            Request::get("/api/v2/operations")
                .header(COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(operations[0]["operation_id"], "op-operator");

        let (status, _, jobs) = request(
            &app,
            Request::get("/api/v2/system/jobs")
                .header(COOKIE, admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(jobs[0]["job_id"], "job-1");
    });
}

#[test]
fn leases_are_exposed_to_viewers() {
    run_async(async {
        let store = InMemoryGuardStore::default();
        store
            .insert_lease(LeaseRecord {
                lease_id: "lease-1".to_string(),
                route_id: "route-1".to_string(),
                resource_id: "stream-1".to_string(),
                node_id: "stream-a".to_string(),
                instance_id: "instance-a".to_string(),
                idempotency_key: "lease-ui-1".to_string(),
                state: LeaseState::Confirmed,
                expires_at_ms: 10_000,
            })
            .unwrap();
        let app = test_app(store);
        let (cookie, _) = login(&app, "viewer").await;
        let (status, _, body) = request(
            &app,
            Request::get("/api/v2/leases")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["lease_id"], "lease-1");
        assert_eq!(body[0]["state"], "confirmed");
    });
}

#[test]
fn event_cursor_polling_is_exposed_over_http() {
    run_async(async {
        let store = InMemoryGuardStore::default();
        for event_id in ["0001", "0002", "0003"] {
            store
                .insert_event_once(EventRecord {
                    event_id: event_id.to_string(),
                    topic: "node.health".to_string(),
                    priority: 1,
                    payload: br#"{"state":"ready"}"#.to_vec(),
                })
                .unwrap();
        }
        let app = test_app(store);
        let (cookie, _) = login(&app, "viewer").await;
        let (status, _, body) = request(
            &app,
            Request::get("/api/v2/events?after_id=0001&limit=1")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"][0]["event_id"], "0002");
        assert_eq!(body["next_after_id"], "0002");
    });
}

#[test]
fn login_rate_limit_returns_too_many_requests() {
    run_async(async {
        let app = test_app(InMemoryGuardStore::default());
        for attempt in 0..6 {
            let (status, _, _) = request(
                &app,
                Request::post("/api/v2/auth/login")
                    .header(ORIGIN, ORIGIN_VALUE)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "username": "viewer", "password": "wrong" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await;
            if attempt < 5 {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            } else {
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            }
        }
    });
}
