use std::sync::Arc;
use std::time::Duration;
use common::exception::{GlobalResult, TransError};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::{broadcast, Mutex};
use common::tokio::time::{timeout};

use crate::coder::{CodecPayload, FrameData, VideoCodec};
use crate::coder::h264::H264Context;
use crate::container::flv::{flv_h264};
use crate::container::ps::PsPacket;
use crate::general::mode::{HALF_TIME_OUT, Media};
use crate::io::hook_handler::InEvent;
use crate::state::cache;
use crate::trans::demuxer::{DemuxContext};

pub mod flv_muxer;
mod hls_muxer;
mod demuxer;

async fn get_stream_in(in_event_rx: Arc<Mutex<broadcast::Receiver<InEvent>>>) -> GlobalResult<()> {
    //此处其他事件不参与仅需判断：MediaInit与StreamIn。但先有MediaInit后有StreamIn.故只需判断StreamIn
    loop {
        let in_event = in_event_rx.lock().await.recv().await.hand_log(|msg| error!("{msg}"))?;
        match in_event {
            InEvent::MediaInit() => {}
            InEvent::StreamIn() => { break; }
        }
    }
    Ok(())
}

//todo 动态自适应编码切换
pub async fn run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        if let Some(in_event_rx) = cache::get_in_event_shard_rx(&ssrc) {
            match timeout(Duration::from_millis(HALF_TIME_OUT), get_stream_in(in_event_rx)).await {
                Ok(res) => {
                    match res {
                        Ok(()) => {
                            if let Some((media, half_channel)) = cache::get_rx_media_type(&ssrc) {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let mut demux_context = DemuxContext::init(ssrc, half_channel.rtp_rx);
                                    let mut codec_payload = CodecPayload::default();
                                    let mut flv_h264_context = flv_h264::MediaFlvContext::register(half_channel.flv_tx);
                                    match media {
                                        Media::PS => {
                                            let mut ps_packet = PsPacket::default();
                                            loop {
                                                if let Err(_) = demux_context.demux_packet(&mut codec_payload, &mut ps_packet) {
                                                    break;
                                                }
                                                if let (Some(codec), vec, ts) = &mut codec_payload.video_payload {
                                                    match codec {
                                                        VideoCodec::H264 => {
                                                            flv_h264_context.packet(vec, *ts);
                                                        }
                                                        VideoCodec::H265 => {}
                                                    }
                                                }
                                            }
                                        }
                                        Media::H264 => {
                                            codec_payload.video_payload.0 = Some(VideoCodec::H264);
                                            let mut h264context = H264Context::init_avc();
                                            loop {
                                                if let Err(_) = demux_context.demux_packet(&mut codec_payload, &mut h264context) {
                                                    break;
                                                }
                                                let (_, vec, ts) = &mut codec_payload.video_payload;
                                                flv_h264_context.packet(vec, *ts);
                                            }
                                        }
                                    }
                                }).await.hand_log(|msg| error!("{msg}"));
                            }
                        }
                        Err(_error) => {
                            error!("接收外部事件发送端drop");
                        }
                    }
                }
                Err(_) => {
                    error!("ssrc = {} 获取媒体初始化信息超时",ssrc);
                }
            }
        }
    }
}
