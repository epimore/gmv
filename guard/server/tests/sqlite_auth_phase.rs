use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use base::serde_json::{Value, json};
use guard::api::v2::ApiV2;
use guard::api::v2::http::{HttpState, router};
use guard::app_config::GuardAppConfig;
use guard::auth::{AuthState, SessionPolicy};
use guard::job::SystemJobService;
use guard::operation::OperationService;
use guard::store::InMemoryGuardStore;
use guard::store::persistent::PersistentStore;
use tower::ServiceExt;

const ORIGIN_VALUE: &str = "http://127.0.0.1:18080";

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

async fn login(app: &axum::Router, username: &str, password: &str) -> (StatusCode, String, String) {
    let (status, headers, body) = call(
        app,
        Request::post("/api/v2/auth/login")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "username": username, "password": password }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    let cookie = headers
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(";")
        .next()
        .unwrap_or_default()
        .to_string();
    let csrf = body
        .get("csrf_token")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    (status, cookie, csrf)
}

fn write_request(
    path: &str,
    cookie: &str,
    csrf: &str,
    body: base::serde_json::Value,
) -> Request<Body> {
    Request::post(path)
        .header(ORIGIN, ORIGIN_VALUE)
        .header(COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[test]
fn sqlite_users_drive_login_roles_session_revocation_and_disable() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let root = std::env::temp_dir().join(format!("guard-auth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let db_path = root.join("guard.db");
            let config_path = root.join("config.yml");
            std::fs::write(
                &config_path,
                format!(
                    r#"db:
  mysql:
    host_or_ip: 127.0.0.1
    port: 3306
    db_name: gmv
    user: gmv
    pass_crypto_enable: false
    pass: ""
    attrs:
      log_global_sql_level: debug
      log_slow_sql_timeout: 30
      timezone: "+8:00"
      charset: utf8mb4
      ssl_level: 0
    pool:
      max_connections: 10
      min_connections: 0
      connection_timeout: 8
      max_lifetime: 1800
      idle_timeout: 60
      check_health: false

log:
  level: info
  prefix: guard-auth-test
  store_path: {}

guard:
  http:
    bind_addr: 127.0.0.1:18080
    origins:
      - {}
    tls:
      enabled: false
  database:
    backend: sqlite
    auto_migrate: true
    pool:
      max_connections: 1
      min_connections: 0
    sqlite:
      path: {}
  bootstrap:
    admin:
      username: admin
      pass_crypto_enable: false
      pass: admin-secret
      local_login_only: true
"#,
                    root.join("logs").display(),
                    ORIGIN_VALUE,
                    db_path.display()
                ),
            )
            .unwrap();

            let config = GuardAppConfig::load(config_path.to_string_lossy().into_owned());
            let persistent = PersistentStore::connect(&config).await.unwrap();
            persistent.initialize(&config).await.unwrap();
            let users = persistent.load_users().await.unwrap();
            let memory = InMemoryGuardStore::default();
            let app = router(HttpState {
                api: ApiV2::new(
                    memory,
                    OperationService::default(),
                    SystemJobService::default(),
                ),
                auth: AuthState::new(
                    users,
                    SessionPolicy {
                        allowed_origins: vec![ORIGIN_VALUE.to_string()],
                        secure_cookie: false,
                        session_ttl: Duration::from_secs(3600),
                        login_window: Duration::from_secs(60),
                        max_failed_attempts: 5,
                        local_admin_username: None,
                        local_admin_login_only: false,
                    },
                ),
                outbox: persistent.outbox_repository(),
                simulator: None,
                users: Some(persistent.user_repository()),
            });

            let (status, admin_cookie, admin_csrf) = login(&app, "admin", "admin-secret").await;
            assert_eq!(status, StatusCode::OK);

            let (status, _, users) = call(
                &app,
                Request::get("/api/v2/users")
                    .header(COOKIE, &admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(users[0]["username"], "admin");
            assert_eq!(users[0]["role"], "admin");

            let (status, _, created) = call(
                &app,
                write_request(
                    "/api/v2/users",
                    &admin_cookie,
                    &admin_csrf,
                    json!({
                        "username": "ops",
                        "role": "operator",
                        "password": "ops-secret",
                        "nickname": "值班员",
                        "enabled": true
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(created["role"], "operator");
            assert_eq!(created["nickname"], "值班员");

            let (status, ops_cookie, _) = login(&app, "ops", "ops-secret").await;
            assert_eq!(status, StatusCode::OK);
            let (status, _, session) = call(
                &app,
                Request::get("/api/v2/auth/session")
                    .header(COOKIE, &ops_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(session["role"], "operator");
            assert_eq!(session["nickname"], "值班员");

            let (status, _, updated) = call(
                &app,
                write_request(
                    "/api/v2/users/ops",
                    &admin_cookie,
                    &admin_csrf,
                    json!({
                        "role": "viewer",
                        "password": "viewer-secret",
                        "nickname": "观察员",
                        "enabled": true
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(updated["role"], "viewer");
            assert_eq!(updated["nickname"], "观察员");

            let (status, _, _) = call(
                &app,
                Request::get("/api/v2/auth/session")
                    .header(COOKIE, &ops_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                login(&app, "ops", "ops-secret").await.0,
                StatusCode::UNAUTHORIZED
            );
            let (status, viewer_cookie, viewer_csrf) = login(&app, "ops", "viewer-secret").await;
            assert_eq!(status, StatusCode::OK);
            let (status, _, _) = call(
                &app,
                write_request(
                    "/api/v2/operations",
                    &viewer_cookie,
                    &viewer_csrf,
                    json!({
                        "operation_id": "viewer-op",
                        "kind": "stream.stop",
                        "dangerous": false,
                        "confirmation": null
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);

            let (status, _, profile) = call(
                &app,
                write_request(
                    "/api/v2/me",
                    &viewer_cookie,
                    &viewer_csrf,
                    json!({
                        "nickname": "我的昵称",
                        "password": "self-secret"
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(profile["nickname"], "我的昵称");
            let (status, _, session) = call(
                &app,
                Request::get("/api/v2/auth/session")
                    .header(COOKIE, &viewer_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(session["nickname"], "我的昵称");
            assert_eq!(
                login(&app, "ops", "viewer-secret").await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(login(&app, "ops", "self-secret").await.0, StatusCode::OK);

            let (status, _, disabled) = call(
                &app,
                write_request(
                    "/api/v2/users/ops",
                    &admin_cookie,
                    &admin_csrf,
                    json!({
                        "role": "viewer",
                        "password": null,
                        "enabled": false
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(disabled["enabled"], false);
            assert_eq!(
                login(&app, "ops", "self-secret").await.0,
                StatusCode::UNAUTHORIZED
            );

            drop(persistent);
            let _ = std::fs::remove_dir_all(root);
        });
}
