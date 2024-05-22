use log::error;
use streamhub::define::FrameData;
use common::err::TransError;

use common::tokio;
use common::tokio::sync::mpsc::{Receiver, unbounded_channel};

mod gb_process;
mod flv_process;

pub async fn run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        let (tx, rx) = unbounded_channel::<FrameData>();
        tokio::spawn(async move {
            let _ = gb_process::run(ssrc, tx).await.hand_err(|msg|error!("{msg}"));
        });
        tokio::spawn(async move {
            let _ = flv_process::run(ssrc, rx).await.hand_err(|msg|error!("{msg}"));
        });
    }
}