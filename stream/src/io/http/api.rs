use crate::io::http::{res_by_code, res_by_error};
use crate::io::local::mp4::Mp4OutputInnerEvent;
use crate::state::register::Register;
use crate::io::talk::TalkManager;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, Router};
use base::err::BaseErrorCode;
use base::exception::GlobalResultExt;
use base::log::{debug, error, info, warn};
use base::serde_json::json;
use base::tokio::sync::mpsc::Sender;
use base::tokio::sync::oneshot;
use futures_util::StreamExt;
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{
    CLOSE_OUTPUT, LISTEN_MEDIA, RECORD_INFO, SDP_MEDIA, STREAM_ONLINE, StreamInfoQo, StreamKey,
    StreamRecordInfo, TALK_ANSWER, TALK_CLOSE, TALK_INPUT_PATH, TALK_OPEN, TalkAnswerReq,
    TalkCloseReq, TalkOpenReq, TalkOpenResp,
};
use shared::info::output::OutputEnum;
use shared::info::res::{EmptyResponse, Resp};
use std::collections::HashMap;
use crate::general::util::dump;

pub fn routes(tx: Sender<u32>) -> Router {
    Router::new()
        .route(
            LISTEN_MEDIA,
            axum::routing::post(listen_media).layer(Extension(tx.clone())),
        )
        .route(SDP_MEDIA, axum::routing::post(sdp_media))
        .route(STREAM_ONLINE, axum::routing::post(stream_online))
        .route(RECORD_INFO, axum::routing::post(record_info))
        .route(CLOSE_OUTPUT, axum::routing::post(close_output))
        .route(TALK_OPEN, axum::routing::post(talk_open))
        .route(TALK_ANSWER, axum::routing::post(talk_answer))
        .route(TALK_CLOSE, axum::routing::post(talk_close))
        .route(TALK_INPUT_PATH, axum::routing::get(talk_input_ws))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/listen/media",
    request_body = MediaConfig,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "媒体流操作"
))]
/// 1.媒体流监听
async fn listen_media(
    Extension(tx): Extension<Sender<u32>>,
    Json(config): Json<MediaConfig>,
) -> Json<Resp<()>> {
    info!("listen_media: {:?}", &config);
    let json = match Register::init_media(config) {
        Ok(ssrc) => match tx.try_send(ssrc).hand_log(|msg| error!("{msg}")) {
            Ok(_) => Resp::<()>::build_success(),
            Err(err) => res_by_error(err),
        },
        Err(err) => res_by_error(err),
    };
    info!("listen_media response: {:?}", &json);
    Json(json)
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/sdp/media",
    request_body = MediaMap,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "媒体流操作"
))]
/// 2.媒体流SDP信息
async fn sdp_media(Json(sdp): Json<MediaMap>) -> Json<Resp<()>> {
    info!("sdp_media: {:?}", &sdp);
    let json = match Register::init_media_ext(sdp.ssrc, sdp.ext) {
        Ok(_) => Resp::<()>::build_success(),
        Err(err) => res_by_error(err),
    };
    info!("sdp_media response: {:?}", &json);
    Json(json)
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/stream/online",
    request_body = StreamKey,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    tag = "媒体流操作"
))]
/// 查看媒体流是否在线
async fn stream_online(Json(stream_key): Json<StreamKey>) -> Json<Resp<bool>> {
    info!("stream_online: {:?}", &stream_key);
    let json = Json(Resp::<bool>::build_success_data(Register::is_exist(
        stream_key,
    )));
    info!("stream_online response: {:?}", &json);
    json
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/record/info",
    request_body = StreamInfoQo,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<StreamRecordInfo>),
        (status = 401, description = "Token无效", body = Resp<StreamRecordInfo>),
        (status = 500, description = "服务器内部错误", body = Resp<StreamRecordInfo>)
    ),
    tag = "媒体流操作"
))]
/// 查看录制进度信息
async fn record_info(Json(info): Json<StreamInfoQo>) -> Json<Resp<StreamRecordInfo>> {
    info!("record_info: {:?}", &info);
    match info.output_enum {
        OutputEnum::LocalMp4 => {
            let (tx, rx) = oneshot::channel();
            if let Ok(_) = Register::try_publish_mpsc::<Mp4OutputInnerEvent>(
                info.ssrc,
                Mp4OutputInnerEvent::StoreInfo(tx),
            ) {
                if let Ok(record) = rx.await {
                    let json = Json(Resp::<StreamRecordInfo>::build_success_data(record));
                    info!("record_info response: {:?}", &json);
                    return json;
                }
            }
        }
        OutputEnum::LocalTs => {}
        _ => {}
    }
    let json = Json(res_by_code::<StreamRecordInfo>(BaseErrorCode::NotFound));
    info!("record_info response: {:?}", &json);
    json
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/close/output",
    request_body = StreamInfoQo,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    tag = "媒体流操作"
))]
///主动关闭录制
async fn close_output(Json(output): Json<StreamInfoQo>) -> Json<Resp<()>> {
    info!("close_output: {:?}", &output);
    match output.output_enum {
        OutputEnum::LocalMp4 => {
            let _ = Register::try_publish_mpsc::<Mp4OutputInnerEvent>(
                output.ssrc,
                Mp4OutputInnerEvent::Close,
            );
        }
        OutputEnum::LocalTs => {}
        _ => {}
    }
    let json = Json(Resp::<()>::build_success());
    info!("close_output response: {:?}", &json);
    json
}

async fn talk_open(Json(req): Json<TalkOpenReq>) -> Json<Resp<TalkOpenResp>> {
    info!("talk_open: {:?}", &req);
    let json = match TalkManager::open(req).await {
        Ok(data) => Resp::build_success_data(data),
        Err(err) => res_by_error(err),
    };
    info!("talk_open response: {:?}", &json);
    Json(json)
}

async fn talk_answer(Json(req): Json<TalkAnswerReq>) -> Json<Resp<()>> {
    info!("talk_answer: {:?}", &req);
    let json = match TalkManager::answer(req) {
        Ok(_) => Resp::<()>::build_success(),
        Err(err) => res_by_error(err),
    };
    info!("talk_answer response: {:?}", &json);
    Json(json)
}

async fn talk_close(Json(req): Json<TalkCloseReq>) -> Json<Resp<()>> {
    info!("talk_close: {:?}", &req);
    TalkManager::close(&req.talk_id);
    let json = Resp::<()>::build_success();
    info!("talk_close response: {:?}", &json);
    Json(json)
}

async fn talk_input_ws(
    Path(talk_id): Path<String>,
    Query(mut query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.remove("gmv-token") else {
        return super::res_401();
    };
    if !TalkManager::check_token(&talk_id, &token) {
        return super::res_401();
    }
    ws.on_upgrade(move |socket| handle_talk_socket(talk_id, socket))
        .into_response()
}

async fn handle_talk_socket(talk_id: String, mut socket: WebSocket) {
    info!("talk websocket opened: talk_id={}", talk_id);
    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(frame)) => {
                // dump("talk",&frame,false).unwrap();
                if let Err(err) = TalkManager::push_frame(&talk_id, frame.to_vec()) {
                    warn!(
                        "talk input frame dropped: talk_id={}, err={:?}",
                        talk_id, err
                    );
                }
            }
            Ok(Message::Text(text)) => {
                debug!(
                    "talk input metadata ignored: talk_id={}, text={}",
                    talk_id, text
                );
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                warn!("talk websocket error: talk_id={}, err={}", talk_id, err);
                break;
            }
        }
    }
    info!("talk websocket closed: talk_id={}", talk_id);
}

async fn open_output_stream(
    Extension(tx): Extension<Sender<u32>>,
    Json(ssrc): Json<u32>,
) -> Resp<()> {
    unimplemented!()
}
async fn close_output_stream(
    Extension(tx): Extension<Sender<u32>>,
    Json(ssrc): Json<u32>,
) -> Resp<()> {
    unimplemented!()
}

async fn open_filter_stream(
    Extension(tx): Extension<Sender<u32>>,
    Json(ssrc): Json<u32>,
) -> Resp<()> {
    unimplemented!()
}
async fn close_filter_stream(
    Extension(tx): Extension<Sender<u32>>,
    Json(ssrc): Json<u32>,
) -> Resp<()> {
    unimplemented!()
}
