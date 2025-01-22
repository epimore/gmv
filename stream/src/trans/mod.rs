use common::log::error;
use common::log::info;
use common::tokio::sync::mpsc::Receiver;

use crate::coder::{FrameData, MediaInfo};
use crate::state::cache;
use crate::trans::demuxer::{DemuxContext, MediaHandler};

pub mod flv_muxer;
mod hls_muxer;
mod demuxer;

pub async fn run(mut rx: Receiver<u32>) {
    let r = rayon::ThreadPoolBuilder::new().build().expect("pics: rayon init failed");
    while let Some(ssrc) = rx.recv().await {
        match cache::get_rx_media_type(&ssrc) {
            None => {
                error!("无效的ssrc = {}",ssrc);
            }
            Some((packet_rx, media_map, flv, hls)) => {
                match (flv, hls) {
                    (true, true) => {
                        let (flv_frame_tx, flv_frame_rx) = crossbeam_channel::unbounded();
                        let (hls_frame_tx, hls_frame_rx) = crossbeam_channel::unbounded();
                        r.spawn(move || {
                            flv_muxer::run(ssrc, flv_frame_rx);
                        });
                        r.spawn(move || {
                            hls_muxer::run(ssrc, hls_frame_rx);
                        });
                        let media_info = MediaInfo::register_all(Some(flv_frame_tx), Some(hls_frame_tx));
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
                    }
                    (true, false) => {
                        let (flv_frame_tx, flv_frame_rx) = crossbeam_channel::unbounded();
                        r.spawn(move || {
                            flv_muxer::run(ssrc, flv_frame_rx);
                        });
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
                    }
                    (false, true) => {
                        let (hls_frame_tx, hls_frame_rx) = crossbeam_channel::unbounded();
                        r.spawn(move || {
                            hls_muxer::run(ssrc, hls_frame_rx);
                        });
                        let media_info = MediaInfo::register_all(None, Some(hls_frame_tx));
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
                    }
                    _ => {
                        error!("ssrc: {} ;无输出端：flv,hls均未开启",ssrc);
                    }
                };
            }
        }
    }
}