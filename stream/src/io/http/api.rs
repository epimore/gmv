use crate::state::cache;
use axum::{Extension, Json, Router};
use common::exception::GlobalResultExt;
use common::log::error;
use common::tokio::sync::mpsc::Sender;
use shared::info::media_initialize::MediaStreamConfig;
use shared::info::media_initialize_ext::MediaMap;
use shared::info::res::Resp;

pub fn routes(tx: Sender<u32>) -> Router {
    Router::new()
        .route("/listen/ssrc", axum::routing::post(stream_init))
        .route("/rtp/media", axum::routing::post(stream_map).layer(Extension(tx.clone())))
}

async fn stream_init(Extension(tx): Extension<Sender<u32>>, Json(config): Json<MediaStreamConfig>) -> Resp<()> {
    match cache::insert_media(config) {
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
    }
}

async fn stream_map(Json(sdp): Json<MediaMap>) -> Resp<()> {
    match cache::insert_media_ext(sdp.ssrc, sdp.ext) {
        Ok(_) => {
            Resp::<()>::build_success()
        }
        Err(err) => {
            Resp::<()>::build_failed_by_msg(err.to_string())
        }
    }
}