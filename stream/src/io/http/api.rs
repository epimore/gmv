use crate::state::cache;
use axum::{Extension, Json, Router};
use common::exception::GlobalResultExt;
use common::log::error;
use common::tokio::sync::mpsc::Sender;
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::res::Resp;
use crate::io::http::{LISTEN_SSRC, RTP_MEDIA};

pub fn routes(tx: Sender<u32>) -> Router {
    Router::new()
        .route(LISTEN_SSRC, axum::routing::post(stream_init))
        .route(RTP_MEDIA, axum::routing::post(stream_map).layer(Extension(tx.clone())))
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