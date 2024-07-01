use std::ops::Deref;
use std::sync::Arc;

use async_channel::RecvError;
use log::{debug, error, info, warn};
use rtp::packet::Packet;

use common::anyhow::anyhow;
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio;
use common::tokio::sync::{broadcast, oneshot};

use crate::{coder, container};
use crate::coder::h264::H264;
use crate::container::rtp::RtpBuffer;
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, tx: broadcast::Sender<FrameData>) -> GlobalResult<()> {
    if let Some(rx) = cache::get_rtp_rx(&ssrc) {
        let rtp_buffer = Arc::new(RtpBuffer::init());
        let produce_buffer = rtp_buffer.clone();
        let (flush_tx, flush_rx) = oneshot::channel();
        tokio::spawn(async move {
            produce_data(ssrc, rx, &produce_buffer, flush_tx).await;
        });
        consume_data(&rtp_buffer, tx, flush_rx).await?;
    }
    Ok(())
}

async fn produce_data(ssrc: u32, rx: async_channel::Receiver<Packet>, rtp_buffer: &RtpBuffer, flush_tx: oneshot::Sender<bool>) {
    while let res_pkt = rx.recv().await {
        match res_pkt {
            Ok(pkt) => { rtp_buffer.insert(pkt).await; }
            Err(_) => {
                let _ = flush_tx.send(true);
                info!("ssrc = {ssrc},流已释放");
                return;
            }
        }
    }
}

async fn consume_data(rtp_buffer: &RtpBuffer, tx: broadcast::Sender<FrameData>, mut flush_rx: oneshot::Receiver<bool>) -> GlobalResult<()> {
    let handle_frame = Box::new(
        move |data: FrameData| -> GlobalResult<()> {
            if let Err(err) = tx.send(data) {
                log::error!("send frame error: {}", err);
            }
            Ok(())
        },
    );
    let mut coder = coder::MediaCoder::register_all(handle_frame);
    loop {
        tokio::select! {
            Some(pkt) = rtp_buffer.next_pkt() =>{
                match pkt.header.payload_type {
                    98 => {}
                    96 => {
                        let _ = coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp);
                    }
                    100 => {}
                    102 => {}
                    _ => {
                        return Err(GlobalError::new_biz_error(4005, "系统暂不支持", |msg| debug!("{msg}")));
                    }
                }
            }
            _ = &mut flush_rx =>{
               for pkt in rtp_buffer.flush_pkt().await{
                     let _ = coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp);
                }
            }
        }
    }
}