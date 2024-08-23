use common::err::TransError;
use common::log::error;
use common::tokio;
use common::tokio::sync::broadcast;
use common::tokio::sync::mpsc::{Receiver};
use crate::coder::FrameData;

use crate::general::mode::BUFFER_SIZE;

mod media_demuxer;
pub mod flv_muxer;
mod hls_muxer;

pub async fn run(mut rx: Receiver<u32>) {
    let media_rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("DEMUXER").build().hand_log(|msg| error!("{msg}")).unwrap();
    let flv_rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("FLV-MUXER").build().hand_log(|msg| error!("{msg}")).unwrap();
    while let Some(ssrc) = rx.recv().await {
        let (frame_tx, frame_rx) = broadcast::channel::<FrameData>(BUFFER_SIZE * 100);
        media_rt.spawn(async move {
            let _ = media_demuxer::run(ssrc, frame_tx).await;
        });
        flv_rt.spawn(async move { flv_muxer::run(ssrc, frame_rx).await; });
    }
}