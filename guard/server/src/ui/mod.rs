use std::path::PathBuf;

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::{CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub fn dist_router(dist_dir: impl Into<PathBuf>) -> Router {
    let dist_dir = dist_dir.into();
    let index = dist_dir.join("index.html");
    secure(
        Router::new()
            .nest_service("/assets", ServeDir::new(dist_dir.join("assets")))
            .fallback_service(ServeFile::new(index)),
    )
}

fn secure(router: Router) -> Router {
    router
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
}

#[cfg(feature = "embed-ui")]
mod embedded {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, StatusCode, Uri, header::CONTENT_TYPE};
    use axum::response::{IntoResponse, Response};
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "../ui/dist/"]
    struct UiAssets;

    pub fn router() -> Router {
        super::secure(Router::new().fallback(asset))
    }

    async fn asset(uri: Uri) -> Response {
        let requested = uri.path().trim_start_matches('/');
        let path = if requested.is_empty() {
            "index.html"
        } else {
            requested
        };
        let (file, response_path) = match UiAssets::get(path) {
            Some(file) => (file, path),
            None if !path.contains('.') => match UiAssets::get("index.html") {
                Some(file) => (file, "index.html"),
                None => return StatusCode::NOT_FOUND.into_response(),
            },
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        let mut response = Response::new(Body::from(file.data));
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static(content_type(response_path)),
        );
        response
    }

    fn content_type(path: &str) -> &'static str {
        match path.rsplit('.').next() {
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("woff2") => "font/woff2",
            _ => "text/html; charset=utf-8",
        }
    }
}

#[cfg(feature = "embed-ui")]
pub use embedded::router as embedded_router;
