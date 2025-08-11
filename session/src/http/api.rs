use crate::state::model::{PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel, PtzControlModel, SingleParam, StreamInfo, StreamNode};
use crate::service::{edge_serv, api_serv};
use axum::http::{HeaderMap, HeaderName};
use axum::{Json, Router};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info};
use shared::info::obj::{StreamRecordInfo, CONTROL_PTZ, DOWNING_INFO, DOWNLOAD_MP4, DOWNLOAD_STOP, PLAY_BACK, PLAY_LIVING, PLAY_SEEK, PLAY_SPEED, RM_FILE};
use shared::info::res::Resp;

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
async fn downing_info(headers: HeaderMap, Json(info): Json<StreamNode>) -> Json<Resp<StreamRecordInfo>> {
    info!("downing_info: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(token) => {
            let stream_id = info.stream_id;
            let stream_server = info.stream_server;
            match api_serv::download_info_by_stream_id(stream_id, stream_server, token).await {
                Ok(data) => { Json(Resp::build_success_data(data)) }
                Err(err) => { Json(Resp::build_failed_by_msg(err.to_string())) }
            }
        }
        Err(_) => {
            Json(Resp::build_failed_by_msg("Gmv-Token is invalid"))
        }
    }
}
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

fn get_gmv_token(headers: HeaderMap) -> GlobalResult<String> {
    let header_name = HeaderName::from_static("gmv-token");
    if let Some(value) = headers.get(&header_name) {
        match value.to_str() {
            Ok(token) => {
                Ok(token.to_string())
            }
            Err(_) => {
                Err(GlobalError::new_biz_error(1100, "Gmv-Token is invalid", |msg| error!("{}", msg)))
            }
        }
    } else {
        Err(GlobalError::new_biz_error(1100, "Gmv-Token not found", |msg| error!("{}", msg)))
    }
}