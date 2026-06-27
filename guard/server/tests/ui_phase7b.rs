use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::http::{Request, StatusCode};
use guard::ui::dist_router;
use tower::ServiceExt;

fn temp_dir() -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("guard-ui-test-{}-{unique}", std::process::id()))
}

#[test]
fn dist_router_serves_assets_and_spa_fallback() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let root = temp_dir();
            fs::create_dir_all(root.join("assets")).unwrap();
            fs::write(root.join("index.html"), "<html>GMV</html>").unwrap();
            fs::write(root.join("assets/app.js"), "console.log('gmv')").unwrap();
            let app = dist_router(&root);

            let response = app
                .clone()
                .oneshot(Request::get("/assets/app.js").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().contains_key(CONTENT_SECURITY_POLICY));

            let response = app
                .oneshot(Request::get("/dashboard").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body.as_ref(), b"<html>GMV</html>");
            fs::remove_dir_all(root).unwrap();
        });
}
