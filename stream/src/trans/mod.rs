use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::{broadcast, Mutex};
use common::tokio::time::{timeout};

use crate::coder::{CodecPayload, FrameData, VideoCodec};
use crate::coder::h264::H264Context;
use crate::container::flv::{flv_h264};
use crate::container::mp4::mp4_h264;
use crate::container::PacketWriter;
use crate::container::ps::PsPacket;
use crate::general::mode::{HALF_TIME_OUT, Media};
use crate::io::hook_handler::{Download, InEvent, MediaAction, Play, RtpStreamEvent};
use crate::state::cache;
use crate::trans::demuxer::{DemuxContext};

pub mod flv_muxer;
mod hls_muxer;
mod demuxer;

async fn get_stream_in(in_event_rx: Arc<Mutex<broadcast::Receiver<InEvent>>>) -> GlobalResult<()> {
    loop {
        let in_event = in_event_rx.lock().await.recv().await.hand_log(|msg| error!("{msg}"))?;
        if let InEvent::RtpStreamEvent(RtpStreamEvent::StreamIn) = in_event {
            break;
        }
    }
    Ok(())
}

pub fn trans_run(rx: Receiver<u32>) {
    std::thread::spawn(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("TRANS_RUN")
            .build()
            .hand_log(|msg| error!("{msg}"))
            .unwrap()
            .block_on(spilt_out_container(rx));
    });
}

async fn spilt_out_container(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        if let Some(in_event_rx) = cache::get_in_event_shard_rx(&ssrc) {
            match timeout(Duration::from_millis(HALF_TIME_OUT), get_stream_in(in_event_rx)).await {
                Ok(res) => {
                    match res {
                        Ok(()) => {
                            if let Some((media, half_channel, stream_id, media_type)) = cache::get_rx_media_type(&ssrc) {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let demux_context = DemuxContext::init(ssrc, half_channel.rtp_rx);
                                    let codec_payload = CodecPayload::default();
                                    match media_type {
                                        MediaAction::Play(Play::Flv) => {
                                            let writer = flv_h264::MediaFlvContext::register(half_channel.flv_tx);
                                            do_remuxer(demux_context, codec_payload, media, writer);
                                        }
                                        MediaAction::Play(Play::Hls(_)) => { unimplemented!() }
                                        MediaAction::Play(Play::FlvHls(_)) => { unimplemented!() }
                                        MediaAction::Download(Download::Mp4(storage_path, _format)) => {
                                            if let Ok(_) = std::fs::create_dir_all(&storage_path).hand_log(|msg| error!("{msg}")) {
                                                if let Ok(file_name) = Path::new(&storage_path).join(format!("{}.mp4", stream_id)).to_str()
                                                    .ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}"))) {
                                                    if let Ok(writer) = mp4_h264::MediaMp4Context::register(half_channel.down_tx, file_name.to_string()) {
                                                        cache::update_down_action_run_by_ssrc(ssrc, true);
                                                        do_remuxer(demux_context, codec_payload, media, writer);
                                                    }
                                                }
                                            }
                                        }
                                        MediaAction::Download(Download::Picture(_file_name, _format)) => { unimplemented!() }
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

fn do_remuxer<W: PacketWriter>(mut demux_context: DemuxContext,
                               mut codec_payload: CodecPayload,
                               media: Media,
                               mut writer: W) {
    match media {
        Media::PS => {
            let mut ps_packet = PsPacket::default();
            loop {
                if demux_context.demux_packet(&mut codec_payload, &mut ps_packet).is_err() {
                    break;
                }
                if let (Some(VideoCodec::H264), vec, ts) = &mut codec_payload.video_payload {
                    writer.packet(vec, *ts);
                }
            }
            writer.packet_end();
        }
        Media::H264 => {
            codec_payload.video_payload.0 = Some(VideoCodec::H264);
            let mut h264context = H264Context::init_avc();
            loop {
                if demux_context.demux_packet(&mut codec_payload, &mut h264context).is_err() {
                    break;
                }
                let (_, vec, ts) = &mut codec_payload.video_payload;
                writer.packet(vec, *ts);
            }
            writer.packet_end();
        }
    }
}

//todo 动态自适应编码切换
/*async fn handle_run(mut rx: Receiver<u32>) {
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
}*/
