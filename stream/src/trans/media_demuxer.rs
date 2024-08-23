use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::log::{error, warn};
use rtp::packet::Packet;

use common::err::{BizError, GlobalError, GlobalResult};
use common::tokio;
use common::tokio::sync::{broadcast, oneshot};
use common::tokio::time::timeout;

use crate::{coder};
use crate::coder::MediaInfo;
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
            produce_data(rx, &produce_buffer, flush_tx).await;
        });
        match cache::get_media_type(&ssrc) {
            None => {
                return Err(GlobalError::new_biz_error(1100, &format!("ssrc = {:?},媒体映射：未查询到ssrc", ssrc), |msg| error!("{msg}")))
            }
            Some((nt, mut map)) => {
                if map.is_empty() {
                    if let Ok(()) = timeout(Duration::from_secs(2), nt.notified()).await {
                        map = cache::get_media_type(&ssrc)
                            .ok_or_else(|| GlobalError::new_sys_error(&format!("ssrc = {ssrc},已释放"),|msg| error!("{msg}")))?.1;
                    }
                }
                consume_data(&rtp_buffer, tx, flush_rx, map).await;
            }
        }
    }
    Ok(())
}

async fn produce_data(rx: async_channel::Receiver<Packet>, rtp_buffer: &RtpBuffer, flush_tx: oneshot::Sender<bool>) {
    loop {
        let res_pkt = rx.recv().await;
        match res_pkt {
            Ok(pkt) => {
                rtp_buffer.insert(pkt);
            }
            Err(_) => {
                let _ = flush_tx.send(true);
                return;
            }
        }
    }
}

async fn consume_data(rtp_buffer: &RtpBuffer, tx: broadcast::Sender<FrameData>, mut flush_rx: oneshot::Receiver<bool>, media_map: HashMap<u8, Media>) {
    let mut coder = coder::MediaInfo::register_all(tx);
    loop {
        tokio::select! {
            res_pkt = rtp_buffer.next_pkt() =>{
                if let Some(pkt) = res_pkt{
                    if let Err(GlobalError::BizErr(BizError { code: 1199, .. }))  = demux_data(&mut coder,pkt,&media_map){
                        break;
                    }
                }
            }
            _ = &mut flush_rx =>{
               for pkt in rtp_buffer.flush_pkt(){
                     if let Err(GlobalError::BizErr(BizError { code: 1199, .. }))  = demux_data(&mut coder,pkt,&media_map){
                        break;
                    }
                }
                break;
            }
        }
    }
}

fn demux_data(coder: &mut MediaInfo, pkt: Packet, media_map: &HashMap<u8, Media>) -> GlobalResult<()> {
    let media_type = pkt.header.payload_type;
    if let Some(media) = media_map.get(&media_type) {
        match *media {
            Media::PS => {
                coder.ps.handle_demuxer(pkt.header.marker, pkt.header.timestamp, pkt.payload)
            }
            Media::H264 => {
                coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp)
            }
        }
    } else {
        match media_type {
            98 => {
                coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp)
            }
            96 => {
                coder.ps.handle_demuxer(pkt.header.marker, pkt.header.timestamp, pkt.payload)
            }
            other => {
                Err(GlobalError::new_biz_error(1199, &format!("系统暂不支持RTP负载类型:{other}"), |msg| warn!("{msg}")))
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

    #[test]
    fn parse_rfind() {
        let uri = "http://172.18.38.186:18570/s1/4FEqqz1Dqsq0Vzqq3K2m0tqq4Zqq6m0s.flv";
        let index = uri.rfind('.').unwrap();
        println!("{}", &uri[index..]);
    }
}