use crate::io::local::mp4::Mp4OutputInnerEvent;
use crate::state::cache;
use axum::{Extension, Json, Router};
use base::exception::GlobalResultExt;
use base::log::{error, info};
use base::tokio::sync::mpsc::Sender;
use base::tokio::sync::oneshot;
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{LISTEN_MEDIA, RECORD_INFO, SDP_MEDIA, STREAM_ONLINE, StreamInfoQo, StreamKey, StreamRecordInfo, CLOSE_OUTPUT};
use shared::info::output::OutputEnum;
use shared::info::res::{EmptyResponse, Resp};

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
    let json = match cache::init_media(config) {
        Ok(ssrc) => match tx.try_send(ssrc).hand_log(|msg| error!("{msg}")) {
            Ok(_) => Resp::<()>::build_success(),
            Err(err) => Resp::<()>::build_failed_by_msg(err.to_string()),
        },
        Err(err) => Resp::<()>::build_failed_by_msg(err.to_string()),
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
    let json = match cache::init_media_ext(sdp.ssrc, sdp.ext) {
        Ok(_) => Resp::<()>::build_success(),
        Err(err) => Resp::<()>::build_failed_by_msg(err.to_string()),
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
    let json = Json(Resp::<bool>::build_success_data(cache::is_exist(
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
            if let Ok(_) = cache::try_publish_mpsc::<Mp4OutputInnerEvent>(
                &info.ssrc,
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
    let json = Json(Resp::<StreamRecordInfo>::build_failed_by_msg(
        "Failed to query record info",
    ));
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
            let _ = cache::try_publish_mpsc::<Mp4OutputInnerEvent>(
                &output.ssrc,
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
