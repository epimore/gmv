use axum::extract::Path;
use axum::Router;

pub fn routes(node: &String) -> Router {
    Router::new().route(&format!("/{node}/:stream_id.flv"), axum::routing::post(flv_handler))
}

async fn flv_handler(Path(stream_id): Path<String>) -> &'static str {
    "FLV stream here"
}