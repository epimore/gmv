use log::error;

use common::err::TransError;
use common::tokio;
use common::tokio::sync::broadcast;
use common::tokio::sync::mpsc::{Receiver};
use crate::coder::FrameData;

use crate::general::mode::BUFFER_SIZE;

mod gb_process;
pub mod flv_process;
mod hls_process;

pub async fn run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        let (tx, _rx) = broadcast::channel::<FrameData>(BUFFER_SIZE);
        let sender = tx.clone();
        tokio::spawn(async move {
            let _ = gb_process::run(ssrc, sender).await.hand_log(|msg| error!("{msg}"));
        });
        let flv_rx = tx.subscribe();
        tokio::spawn(async move {
            flv_process::run(ssrc, flv_rx).await;
        });
        // let hls_rx = tx.subscribe();
        // tokio::spawn(async move {
        //     let _ = hls_process::run(ssrc, hls_rx).await.hand_log(|msg| error!("{msg}"));
        // });
    }
}