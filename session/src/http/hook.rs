use axum::{Json, Router};
use base::log::info;
use shared::info::obj::{
    END_RECORD, INPUT_TIMEOUT, InTimeoutEventRes, OFF_PLAY, ON_PLAY, OutputEventRes,
    OutputStreamInfo, RegisterStreamInfo, STREAM_IDLE, STREAM_REGISTER, StreamPlayInfo,
    StreamRecordInfo, StreamState, TALK_CLOSED, TalkClosedEvent,
};
use shared::info::res::{EmptyResponse, Resp};

use crate::service::hook_serv;

pub fn routes() -> Router {
    Router::new()
        .route(STREAM_REGISTER, axum::routing::post(stream_register))
        .route(INPUT_TIMEOUT, axum::routing::post(stream_input_timeout))
        .route(ON_PLAY, axum::routing::post(on_play))
        .route(STREAM_IDLE, axum::routing::post(stream_idle))
        .route(OFF_PLAY, axum::routing::post(off_play))
        .route(END_RECORD, axum::routing::post(end_record))
        .route(TALK_CLOSED, axum::routing::post(talk_closed))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/register",
    request_body = RegisterStreamInfo,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn stream_register(Json(info): Json<RegisterStreamInfo>) -> Json<Resp<()>> {
    info!("stream_register = {:?}", &info);
    hook_serv::stream_register(info).await;
    Json(Resp::build_success())
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/input/timeout",
    request_body = StreamState,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<InTimeoutEventRes>),
        (status = 401, description = "Token无效", body = Resp<InTimeoutEventRes>),
        (status = 500, description = "服务器内部错误", body = Resp<InTimeoutEventRes>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn stream_input_timeout(Json(info): Json<StreamState>) -> Json<Resp<InTimeoutEventRes>> {
    info!("stream_input_timeout = {:?}", &info);
    Json(Resp::build_success_data(hook_serv::stream_input_timeout(
        info,
    )))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/on/play",
    request_body = StreamPlayInfo,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn on_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<bool>> {
    info!("on_play = {:?}", &info);
    Json(Resp::<bool>::build_success_data(hook_serv::on_play(info)))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/off/play",
    request_body = StreamPlayInfo,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn off_play(Json(info): Json<StreamPlayInfo>) -> Json<Resp<()>> {
    info!("off_play = {:?}", &info);
    hook_serv::off_play(info).await;
    Json(Resp::build_success())
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/stream/idle",
    request_body = OutputStreamInfo,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<OutputEventRes>),
        (status = 401, description = "Token无效", body = Resp<OutputEventRes>),
        (status = 500, description = "服务器内部错误", body = Resp<OutputEventRes>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn stream_idle(Json(info): Json<OutputStreamInfo>) -> Json<Resp<OutputEventRes>> {
    info!("stream_idle = {:?}", &info);
    Json(Resp::build_success_data(hook_serv::stream_idle(info).await))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/end/record",
    request_body = StreamRecordInfo,
    responses(
        (status = 200, description = "回调处理成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "流媒体服务回调接口"
))]
async fn end_record(Json(info): Json<StreamRecordInfo>) -> Json<Resp<()>> {
    info!("end_record = {:?}", &info);
    match hook_serv::end_record(info).await {
        Ok(()) => Json(Resp::build_success()),
        Err(err) => Json(crate::http::res_by_error(err)),
    }
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/hook/talk/closed",
    request_body = TalkClosedEvent,
    responses(
        (status = 200, description = "å›žè°ƒå¤„ç†æˆåŠŸ", body = Resp<bool>),
        (status = 500, description = "æœåŠ¡å™¨å†…éƒ¨é”™è¯¯", body = Resp<bool>)
    ),
    tag = "æµåª’ä½“æœåŠ¡å›žè°ƒæŽ¥å£"
))]
async fn talk_closed(Json(info): Json<TalkClosedEvent>) -> Json<Resp<bool>> {
    info!("talk_closed = {:?}", &info);
    Json(Resp::build_success_data(hook_serv::talk_closed(info).await))
}
