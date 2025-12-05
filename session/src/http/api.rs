use crate::state::model::{PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel, PtzControlModel, StreamInfo, StreamQo};
use crate::service::{edge_serv, api_serv};
use axum::http::{HeaderMap, HeaderName};
use axum::{Json, Router};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use shared::info::obj::{SingleParam, StreamRecordInfo, CONTROL_PTZ, DOWNING_INFO, DOWNLOAD_MP4, DOWNLOAD_STOP, PLAY_BACK, PLAY_LIVING, PLAY_SEEK, PLAY_SPEED, RM_FILE};
use shared::info::res::{EmptyResponse, Resp};
use crate::http::get_gmv_token;

pub fn routes() -> Router {
    Router::new()
        .route(PLAY_LIVING, axum::routing::post(play_living))
        .route(PLAY_BACK, axum::routing::post(play_back))
        .route(PLAY_SEEK, axum::routing::post(play_seek))
        .route(PLAY_SPEED, axum::routing::post(play_speed))
        .route(CONTROL_PTZ, axum::routing::post(control_ptz))
        .route(DOWNLOAD_MP4, axum::routing::post(download_mp4))
        .route(DOWNLOAD_STOP, axum::routing::post(download_stop))
        .route(DOWNING_INFO, axum::routing::post(downing_info))
        .route(RM_FILE, axum::routing::post(rm_file))
}

#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/play/live/stream",
    request_body = PlayLiveModel,
    responses(
        (status = 200, description = "实时流播放成功", body = Resp<StreamInfo>),
        (status = 401, description = "Token无效", body = Resp<StreamInfo>),
        (status = 500, description = "服务器内部错误", body = Resp<StreamInfo>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 点播实时视频
async fn play_living(headers: HeaderMap, Json(info): Json<PlayLiveModel>) -> Json<Resp<StreamInfo>> {
    info!("play_live: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::play_live(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/play/back/stream",
    request_body = PlayBackModel,
    responses(
        (status = 200, description = "回放流播放成功", body = Resp<StreamInfo>),
        (status = 401, description = "Token无效", body = Resp<StreamInfo>),
        (status = 500, description = "服务器内部错误", body = Resp<StreamInfo>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 点播历史视频
async fn play_back(headers: HeaderMap, Json(info): Json<PlayBackModel>) -> Json<Resp<StreamInfo>> {
    info!("play_back: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::play_back(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/play/seek",
    request_body = PlaySeekModel,
    responses(
        (status = 200, description = "播放跳转成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 历史视频拖动播放
async fn play_seek(headers: HeaderMap, Json(info): Json<PlaySeekModel>) -> Json<Resp<bool>> {
    info!("play_seek: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::seek(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/play/speed",
    request_body = PlaySpeedModel,
    responses(
        (status = 200, description = "播放速度调整成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 历史视频倍速播放
async fn play_speed(headers: HeaderMap, Json(info): Json<PlaySpeedModel>) -> Json<Resp<bool>> {
    info!("play_speed: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::speed(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/control/ptz",
    request_body = PtzControlModel,
    responses(
        (status = 200, description = "云台控制成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 摄像机云台控制
async fn control_ptz(headers: HeaderMap, Json(info): Json<PtzControlModel>) -> Json<Resp<bool>> {
    info!("control_ptz: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::ptz(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
//let recommendations = [
//     ("短视频平台", "1-10分钟，<500MB"),
//     ("在线课程", "15-45分钟，<1GB"),
//     ("电影", "按章节或90-120分钟分片"),
//     ("监控视频", "按1小时或1GB分片"),
//     ("直播录制", "按2-4小时，考虑CDN分片"),
// ];
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/download/mp4",
    request_body = PlayBackModel,
    responses(
        (status = 200, description = "MP4下载任务创建成功", body = Resp<String>),
        (status = 401, description = "Token无效", body = Resp<String>),
        (status = 500, description = "服务器内部错误", body = Resp<String>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 历史视频mp4录制
async fn download_mp4(headers: HeaderMap, Json(info): Json<PlayBackModel>) -> Json<Resp<String>> {
    info!("download_mp4: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::download(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/download/stop",
    request_body = SingleParam<String>,
    responses(
        (status = 200, description = "下载任务停止成功", body = Resp<bool>),
        (status = 401, description = "Token无效", body = Resp<bool>),
        (status = 500, description = "服务器内部错误", body = Resp<bool>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 停止视频录制
async fn download_stop(headers: HeaderMap, Json(info): Json<SingleParam<String>>) -> Json<Resp<bool>> {
    info!("download_stop: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::download_stop(info.param, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/download/info",
    request_body = StreamQo,
    responses(
        (status = 200, description = "获取下载信息成功", body = Resp<StreamRecordInfo>),
        (status = 401, description = "Token无效", body = Resp<StreamRecordInfo>),
        (status = 500, description = "服务器内部错误", body = Resp<StreamRecordInfo>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 查看录制中的视频进度信息
async fn downing_info(headers: HeaderMap, Json(info): Json<StreamQo>) -> Json<Resp<StreamRecordInfo>> {
    info!("downing_info: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            match api_serv::download_info_by_stream_id(info, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
#[cfg_attr(debug_assertions, utoipa::path(
    post,
    path = "/api/file/remove",
    request_body = SingleParam<i64>,
    responses(
        (status = 200, description = "文件删除成功", body = Resp<EmptyResponse>),
        (status = 401, description = "Token无效", body = Resp<EmptyResponse>),
        (status = 500, description = "服务器内部错误", body = Resp<EmptyResponse>)
    ),
    security(
        ("gmv_token" = [])
    ),
    tag = "设备媒体流操作API"
))]
/// 物理删除云端录制的视频
async fn rm_file(headers: HeaderMap, Json(info): Json<SingleParam<i64>>) -> Json<Resp<()>> {
    info!("rm_file: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(_token) => {
            match edge_serv::rm_file(info.param).await {
                Ok(_) => { Json(Resp::build_success()) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}