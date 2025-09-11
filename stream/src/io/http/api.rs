use crate::state::cache;
use axum::{Extension, Json, Router};
use base::exception::GlobalResultExt;
use base::log::{error, info};
use base::tokio::sync::mpsc::Sender;
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{StreamKey, LISTEN_SSRC, RTP_MEDIA, STREAM_ONLINE};
use shared::info::res::Resp;

pub fn routes(tx: Sender<u32>) -> Router {
    Router::new()
        .route(LISTEN_SSRC, axum::routing::post(stream_init).layer(Extension(tx.clone())))
        .route(RTP_MEDIA, axum::routing::post(stream_map))
        .route(STREAM_ONLINE, axum::routing::post(stream_online))
}

async fn stream_init(Extension(tx): Extension<Sender<u32>>, Json(config): Json<MediaStreamConfig>) -> Json<Resp<()>> {
    info!("stream_init: {:?}",&config);
    let json = match cache::init_media(config) {
        Ok(ssrc) => {
            match tx.try_send(ssrc).hand_log(|msg| error!("{msg}")) {
                Ok(_) => { Resp::<()>::build_success() }
                Err(err) => {
                    Resp::<()>::build_failed_by_msg(err.to_string())
                }
            }
        }
        Err(err) => {
            Resp::<()>::build_failed_by_msg(err.to_string())
        }
    };
    info!("stream_init response: {:?}",&json);
    Json(json)
}

async fn stream_map(Json(sdp): Json<MediaMap>) -> Json<Resp<()>> {
    info!("stream_map: {:?}",&sdp);
    let json = match cache::init_media_ext(sdp.ssrc, sdp.ext) {
        Ok(_) => {
            Resp::<()>::build_success()
        }
        Err(err) => {
            Resp::<()>::build_failed_by_msg(err.to_string())
        }
    };
    info!("stream_map response: {:?}",&json);
    Json(json)
}

async fn stream_online(Json(stream_key): Json<StreamKey>) -> Json<Resp<bool>> {
    info!("stream_online: {:?}",&stream_key);
    let json = Json(Resp::<bool>::build_success_data(cache::is_exist(stream_key)));
    info!("stream_online response: {:?}",&json);
    json
}

async fn open_output_stream(Extension(tx): Extension<Sender<u32>>, Json(ssrc): Json<u32>) -> Resp<()> {
    unimplemented!()
}
async fn close_output_stream(Extension(tx): Extension<Sender<u32>>, Json(ssrc): Json<u32>) -> Resp<()> {
    unimplemented!()
}

async fn open_filter_stream(Extension(tx): Extension<Sender<u32>>, Json(ssrc): Json<u32>) -> Resp<()> {
    unimplemented!()
}
async fn close_filter_stream(Extension(tx): Extension<Sender<u32>>, Json(ssrc): Json<u32>) -> Resp<()> {
    unimplemented!()
}