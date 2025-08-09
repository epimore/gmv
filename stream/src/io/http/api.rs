use crate::state::cache;
use axum::{Extension, Json, Router};
use base::exception::GlobalResultExt;
use base::log::error;
use base::tokio::sync::mpsc::Sender;
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{StreamKey, LISTEN_SSRC, RTP_MEDIA, STREAM_ONLINE};
use shared::info::res::Resp;

pub fn routes(tx: Sender<u32>) -> Router {
    Router::new()
        .route(LISTEN_SSRC, axum::routing::post(stream_init))
        .route(RTP_MEDIA, axum::routing::post(stream_map).layer(Extension(tx.clone())))
        .route(STREAM_ONLINE, axum::routing::post(stream_online))
}

async fn stream_init(Extension(tx): Extension<Sender<u32>>, Json(config): Json<MediaStreamConfig>) -> Json<Resp<()>> {
    let json = match cache::insert_media(config) {
        Ok(ssrc) => {
            match tx.send(ssrc).await.hand_log(|msg| error!("{msg}")) {
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
    Json(json)
}

async fn stream_map(Json(sdp): Json<MediaMap>) -> Json<Resp<()>> {
    let json = match cache::insert_media_ext(sdp.ssrc, sdp.ext) {
        Ok(_) => {
            Resp::<()>::build_success()
        }
        Err(err) => {
            Resp::<()>::build_failed_by_msg(err.to_string())
        }
    };
    Json(json)
}

async fn stream_online(Json(stream_key): Json<StreamKey>) -> Json<Resp<bool>> {
    Json(Resp::<bool>::build_success_data(cache::is_exist(stream_key)))
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