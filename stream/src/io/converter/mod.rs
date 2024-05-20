use common::err::{GlobalResult, TransError};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc::Receiver;

mod handler;
mod ff;

pub async fn run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        tokio::spawn(async move {
            let _ = ff::parse(ssrc).hand_err(|msg| error!("{msg}"));
        });
    }
}
