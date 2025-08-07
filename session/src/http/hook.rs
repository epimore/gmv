use crate::service::{handler};
use axum::{Json, Router};
use common::log::info;
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState, END_RECORD, INPUT_TIMEOUT, OFF_PLAY, ON_PLAY, STREAM_IDLE, STREAM_REGISTER};
use shared::info::res::Resp;

pub fn routes() -> Router {
    Router::new()
        .route(STREAM_REGISTER, axum::routing::post(stream_register))
        .route(INPUT_TIMEOUT, axum::routing::post(stream_input_timeout))
        .route(ON_PLAY, axum::routing::post(on_play))
        .route(STREAM_IDLE, axum::routing::post(stream_idle))
        .route(OFF_PLAY, axum::routing::post(off_play))
        .route(END_RECORD, axum::routing::post(end_record))
}

async fn stream_register(Json(info): Json<BaseStreamInfo>) -> Json<Resp<()>> {
    info!("stream_register = {:?}", &info);
    handler::stream_register(info).await;
    Json(Resp::build_success())
}
async fn stream_input_timeout(Json(info): Json<StreamState>) -> Json<Resp<()>> {
    info!("stream_input_timeout = {:?}", &info);
    handler::stream_input_timeout(info);
    Json(Resp::build_success())
}
async fn on_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<bool>> {
    info!("on_play = {:?}", &info);
    Json(Resp::<bool>::build_success_data(handler::on_play(info)))
}
async fn off_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<()>> {
    info!("off_play = {:?}", &info);
    handler::off_play(info).await;
    Json(Resp::build_success())
}
async fn stream_idle(Json(info): Json<BaseStreamInfo>) -> Json<Resp<()>> {
    info!("stream_idle = {:?}", &info);
    handler::stream_idle(info).await;
    Json(Resp::build_success())
}
async fn end_record(Json(info): Json<StreamRecordInfo>) -> Json<Resp<()>> {
    info!("end_record = {:?}", &info);
    handler::end_record(info).await;
    Json(Resp::build_success())
}