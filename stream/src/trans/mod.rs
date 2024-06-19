use log::error;

use common::bytes::{Bytes, BytesMut};
use common::err::TransError;
use common::tokio;
use common::tokio::sync::mpsc::{Receiver, unbounded_channel};

use crate::general::mode::BUFFER_SIZE;

mod gb_process;
mod flv_process;

#[derive(Clone)]
pub enum FrameData {
    Video { timestamp: u32, data: Bytes },
    Audio { timestamp: u32, data: Bytes },
    MetaData { timestamp: u32, data: Bytes },
}

pub async fn run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        let (tx, rx) = crossbeam_channel::bounded::<FrameData>(BUFFER_SIZE);
        tokio::spawn(async move {
            let _ = gb_process::run(ssrc, tx).await.hand_log(|msg| error!("{msg}"));
        });
        tokio::spawn(async move {
            let _ = flv_process::run(ssrc, rx).await.hand_log(|msg| error!("{msg}"));
        });
    }
}