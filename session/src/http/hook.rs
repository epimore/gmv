use crate::service::{hook_serv};
use axum::{Json, Router};
use base::log::info;
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState, END_RECORD, INPUT_TIMEOUT, OFF_PLAY, ON_PLAY, STREAM_IDLE, STREAM_REGISTER};
use shared::info::res::{EmptyResponse, Resp};

pub fn routes() -> Router {
    Router::new()
        .route(STREAM_REGISTER, axum::routing::post(stream_register))
        .route(INPUT_TIMEOUT, axum::routing::post(stream_input_timeout))
        .route(ON_PLAY, axum::routing::post(on_play))
        .route(STREAM_IDLE, axum::routing::post(stream_idle))
        .route(OFF_PLAY, axum::routing::post(off_play))
        .route(END_RECORD, axum::routing::post(end_record))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/register",
    request_body = BaseStreamInfo,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流注册回调接口
async fn stream_register(Json(info): Json<BaseStreamInfo>) -> Json<Resp<()>> {
    info!("stream_register = {:?}", &info);
    hook_serv::stream_register(info).await;
    Json(Resp::build_success())
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/input/timeout",
    request_body = StreamState,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流输入超时回调接口
async fn stream_input_timeout(Json(info): Json<StreamState>) -> Json<Resp<()>> {
    info!("stream_input_timeout = {:?}", &info);
    hook_serv::stream_input_timeout(info);
    Json(Resp::build_success())
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/on/play",
    request_body = StreamPlayInfo,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流播放回调接口
async fn on_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<bool>> {
    info!("on_play = {:?}", &info);
    Json(Resp::<bool>::build_success_data(hook_serv::on_play(info)))
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/off/play",
    request_body = StreamPlayInfo,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流关闭播放回调接口
async fn off_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<()>> {
    info!("off_play = {:?}", &info);
    hook_serv::off_play(info).await;
    Json(Resp::build_success())
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/idle",
    request_body = BaseStreamInfo,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流空闲回调接口
async fn stream_idle(Json(info): Json<BaseStreamInfo>) -> Json<Resp<()>> {
    info!("stream_idle = {:?}", &info);
    hook_serv::stream_idle(info).await;
    Json(Resp::build_success())
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/end/record",
    request_body = StreamRecordInfo,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
/// 媒体流录制完成回调接口
async fn end_record(Json(info): Json<StreamRecordInfo>) -> Json<Resp<()>> {
    info!("end_record = {:?}", &info);
    hook_serv::end_record(info).await;
    Json(Resp::build_success())
}