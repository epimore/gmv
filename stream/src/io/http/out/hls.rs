use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

pub async fn m3u8_handler() -> Response<Body> {
    unsupported_hls()
}

pub async fn segment_ts_handler() -> Response<Body> {
    unsupported_hls()
}

pub async fn segment_mp4_handler() -> Response<Body> {
    unsupported_hls()
}

fn unsupported_hls() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(Body::from("HLS output is not implemented"))
        .expect("valid HLS unsupported response")
}
