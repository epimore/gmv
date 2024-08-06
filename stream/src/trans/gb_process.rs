use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::log::{debug, error, info, warn};
use rtp::packet::Packet;
use common::anyhow::anyhow;

use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio;
use common::tokio::sync::{broadcast, oneshot};
use common::tokio::time::timeout;

use crate::{coder};
use crate::container::rtp::RtpBuffer;
use crate::general::mode::Media;
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
        match cache::get_media_type(&ssrc) {
            None => {
                return Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},媒体映射：未查询到ssrc", ssrc), |msg| error!("{msg}")))
            }
            Some((nt, mut map)) => {
                if map.is_empty() {
                    if let Ok(()) = timeout(Duration::from_secs(2), nt.notified()).await {
                        map = cache::get_media_type(&ssrc)
                            .ok_or_else(|| SysErr(anyhow!("ssrc = {},已释放",ssrc)))?.1;
                    }
                }
                consume_data(&rtp_buffer, tx, flush_rx, map).await?;
            }
        }
    }
    Ok(())
}

async fn produce_data(ssrc: u32, rx: async_channel::Receiver<Packet>, rtp_buffer: &RtpBuffer, flush_tx: oneshot::Sender<bool>) {
    loop {
        let res_pkt = rx.recv().await;
        match res_pkt {
            Ok(pkt) => {
                rtp_buffer.insert(pkt);
            }
            Err(_) => {
                let _ = flush_tx.send(true);
                info!("ssrc = {ssrc},流已释放");
                return;
            }
        }
    }
}

async fn consume_data(rtp_buffer: &RtpBuffer, tx: broadcast::Sender<FrameData>, mut flush_rx: oneshot::Receiver<bool>, media_map: HashMap<u8, Media>) -> GlobalResult<()> {
    let handle_frame = Box::new(
        move |data: FrameData| -> GlobalResult<()> {
            tx.send(data).map_err(|err| SysErr(anyhow!(err.to_string()))).map(|_| ())
        },
    );
    let mut coder = coder::MediaInfo::register_all(handle_frame);
    loop {
        tokio::select! {
            res_pkt = rtp_buffer.next_pkt() =>{
                if let Some(pkt) = res_pkt{
                   let media_type = pkt.header.payload_type;
                    if let Some(media) = media_map.get(&media_type){
                        match *media{
                            Media::PS => {
                                let ts = coder.ps.ts;
                                if let Ok(Some(vec)) = coder.ps.ps_packet.parse(pkt.payload).hand_log(|msg|warn!("{msg}")){
                                   for val in vec{
                                   let _ = coder.h264.handle_demuxer(val, ts).hand_log(|msg|warn!("{msg}"));
                                   }
                                }
                                coder.ps.ts = pkt.header.timestamp;
                            }
                            Media::H264 => {
                                let _ = coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp).hand_log(|msg|warn!("{msg}"));
                            }}
                    }else{
                         match media_type{
                            98 => {
                                let _ = coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp).hand_log(|msg|warn!("{msg}"));
                            }
                            96 => {
                                 let ts = coder.ps.ts;
                                 if let Ok(Some(vec)) = coder.ps.ps_packet.parse(pkt.payload).hand_log(|msg|warn!("{msg}")){
                                    for val in vec{
                                    let _ = coder.h264.handle_demuxer(val, ts).hand_log(|msg|warn!("{msg}"));
                                    }
                                }
                                 coder.ps.ts = pkt.header.timestamp;
                            }
                            _ => {
                                return Err(GlobalError::new_biz_error(4005, "系统暂不支持", |msg| debug!("{msg}")));
                            }
                    }
                    }
                }
            }
            _ = &mut flush_rx =>{
               for pkt in rtp_buffer.flush_pkt(){
                     let _ = coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp).hand_log(|msg|warn!("{msg}"));
                }
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn parse_frame_nal() {
        let tp = 0x7c & 0x1f;
        println!("frame type: nal_ref_idc = {}, type = {}", 0x7c >> 5 & 0x03, tp);
        println!("fu-a type: s = {}, e = {}, type = {}", 0x81 >> 7 & 0x01, 0x81 >> 6 & 0x01, 0x81 & 0x1f);
    }
}