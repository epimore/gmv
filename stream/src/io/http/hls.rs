use axum::{Router, routing::get};

pub fn routes() -> Router {
    Router::new()
        .route("/live.m3u8", get(m3u8_handler))
        .route("/segment.ts", get(segment_handler))
}

async fn m3u8_handler() -> &'static str {
    "#EXTM3U ...\n"
}

async fn segment_handler() -> &'static str {
    "Fake TS segment"
}
