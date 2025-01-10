use log::info;
use common::log::error;
use common::tokio::sync::mpsc::{Receiver};
use crate::coder::{FrameData, MediaInfo};

use crate::state::cache;
use crate::trans::demuxer::{MediaHandler, DemuxContext};

pub mod flv_muxer;
mod hls_muxer;
mod demuxer;

// //rtp数据包 包装类型；当为None时 》》》 已达数据结束
// pub type RtpPacketWrap = Option<Packet>;
// //原始数据帧 包装类型；当为None时 》》》 已达数据结束
// pub type FrameDataWrap = Option<FrameData>;

pub async fn run(mut rx: Receiver<u32>) {
    let r = rayon::ThreadPoolBuilder::new().build().expect("pics: rayon init failed");
    while let Some(ssrc) = rx.recv().await {
        match cache::get_rx_media_type(&ssrc) {
            None => {
                error!("无效的ssrc = {}",ssrc);
            }
            Some((packet_rx, media_map)) => {
                //todo 按需支持hls,此处设置hls为None
                let (flv_frame_tx, flv_frame_rx) = crossbeam_channel::unbounded();
                // let (hls_frame_tx,hls_frame_rx) = crossbeam_channel::unbounded();
                let media_info = MediaInfo::register_all(Some(flv_frame_tx), None);
                let handler = MediaHandler::new(media_map, media_info);
                let mut context = DemuxContext::init(packet_rx, handler);
                r.spawn(move || {
                    loop {
                        if let Err(_error) = context.demux_packet() {
                            info!("ssrc: {ssrc};退出流转换");
                            break;
                        }
                    }
                });
                r.spawn(move || {
                    flv_muxer::run(ssrc, flv_frame_rx);
                });
            }
        }
    }
}