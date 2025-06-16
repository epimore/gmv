use std::mem;
use crate::media::format::muxer::{MuxerEnum, MuxerSink};
use common::bytes::Bytes;
use common::exception::GlobalResult;
use common::log::warn;
use common::tokio::sync::{broadcast, mpsc};
use rsmpeg::ffi::{av_packet_ref, av_packet_unref, av_read_frame, AVPacket};
use crate::media::format::demuxer::{DemuxerContext};
use crate::media::rtp;

pub(super) mod demuxer;
mod muxer;
mod flv;
mod mp4;
mod hls;
pub struct RemuxerContext {
    pub sdp_map: (u8, String),
    pub rtp_buffer: rtp::RtpPacketBuffer,
    pub event_tx: broadcast::Sender<RemuxerOutEvent>,
    pub event_rx: mpsc::Receiver<RemuxerInEvent>,
}
impl RemuxerContext {
    pub fn start_remuxer<T: std::marker::Send>(self, t: T, init_muxer_callback: fn(&DemuxerContext, T) -> MuxerEnum) -> GlobalResult<()>
    {
        let demuxer_context = DemuxerContext::start_demuxer(&self.sdp_map, self.rtp_buffer)?;
        let init_muxer_enum = init_muxer_callback(&demuxer_context, t);
        let mut remuxer_loop = RemuxerLoop::new(init_muxer_enum, demuxer_context, self.event_tx, self.event_rx);
        remuxer_loop.run()?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum MediaBoxType {
    FLV,
    HLS,
    MP4,
}
pub enum MediaBoxInner {
    //is_idr,packet
    FLV(broadcast::Sender<(bool, Bytes)>),
    HLS,
    MP4,
}

pub enum RemuxerInEvent {
    OPEN(MediaBoxInner),
    CLOSE(MediaBoxType),
    HEADER(MediaBoxType),
}
pub enum RemuxerOutEvent {
    FlvHeader(Bytes),
}

pub struct RemuxerLoop {
    muxers: Vec<MuxerEnum>,
    demuxer_context: DemuxerContext,
    event_tx: broadcast::Sender<RemuxerOutEvent>,
    event_rx: mpsc::Receiver<RemuxerInEvent>,
}

impl RemuxerLoop {
    fn run(&mut self) -> GlobalResult<()> {
        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            let mut pkt = std::mem::zeroed::<AVPacket>();

            loop {
                // 处理输入事件
                self.event_in_hook()?;

                // 读取媒体包
                if rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt) < 0 {
                    break;
                }
                pkt.stream_index == 0

                // 克隆包并发送给所有muxer
                let mut cloned_pkt = std::mem::zeroed::<AVPacket>();
                rsmpeg::ffi::av_packet_ref(&mut cloned_pkt, &pkt);

                for muxer in &mut self.muxers {
                    match muxer {
                        MuxerEnum::Flv(flv) => flv.write_packet(&cloned_pkt),
                        MuxerEnum::Hls(hls) => hls.write_packet(&cloned_pkt),
                        MuxerEnum::Mp4(mp4) => mp4.write_packet(&cloned_pkt),
                    }
                }

                rsmpeg::ffi::av_packet_unref(&mut pkt);
                rsmpeg::ffi::av_packet_unref(&mut cloned_pkt);
            }

            // 清理资源
            // for muxer in &mut self.muxers {
            //     match muxer {
            //         MuxerEnum::Flv(flv) => drop(flv),
            //         MuxerEnum::Hls(hls) => drop(hls),
            //         MuxerEnum::Mp4(mp4) => drop(mp4),
            //     }
            // }
            Ok(())
        }
    }

    fn new(init_muxer: MuxerEnum,
           demuxer_context: DemuxerContext,
           event_tx: broadcast::Sender<RemuxerOutEvent>,
           event_rx: mpsc::Receiver<RemuxerInEvent>) -> Self {
        Self {
            muxers: vec![init_muxer],
            demuxer_context,
            event_tx,
            event_rx,
        }
    }

    fn add_muxer<F: Fn() -> GlobalResult<MuxerEnum>>(muxers: &mut Vec<MuxerEnum>, box_type: MediaBoxType, call_new_muxer: F) -> bool {
        if call_new_muxer().is_err() { return false; }
        for muxer in muxers.iter() {
            match (box_type, muxer) {
                (MediaBoxType::FLV, MuxerEnum::Flv(_)) => return false,
                (MediaBoxType::HLS, MuxerEnum::Hls(_)) => return false,
                (MediaBoxType::MP4, MuxerEnum::Mp4(_)) => return false,
                _ => {}
            }
        }
        muxers.push(call_new_muxer().unwrap());
        true
    }

    fn event_in_hook(&mut self) -> GlobalResult<()> {
        if let Ok(event_in) = self.event_rx.try_recv() {
            match event_in {
                RemuxerInEvent::OPEN(media_box) => {
                    match media_box {
                        MediaBoxInner::FLV(flv_tx) => {
                            let flv_muxer_enum = || {
                                let flv_muxer = flv::FlvMuxer::new(flv_tx, &self.demuxer_context)?;
                                Ok(MuxerEnum::Flv(flv_muxer))
                            };
                            if !Self::add_muxer(&mut self.muxers, MediaBoxType::FLV, flv_muxer_enum) {
                                warn!("flv muxer already exist");
                            }
                        }
                        MediaBoxInner::HLS => {}
                        MediaBoxInner::MP4 => {}
                    }
                }
                RemuxerInEvent::CLOSE(mbt) => {
                    self.muxers.retain(|muxer| match (mbt, muxer) {
                        (MediaBoxType::FLV, MuxerEnum::Flv(_)) => false,
                        (MediaBoxType::HLS, MuxerEnum::Hls(_)) => false,
                        (MediaBoxType::MP4, MuxerEnum::Mp4(_)) => false,
                        _ => true,  // 其他情况保留
                    })
                }
                RemuxerInEvent::HEADER(header) => {
                    match header {
                        MediaBoxType::FLV => {
                            for muxer in &self.muxers {
                                if let MuxerEnum::Flv(flv) = muxer {
                                    let out_event = RemuxerOutEvent::FlvHeader(flv.get_header());
                                    let _ = self.event_tx.send(out_event);
                                    break;
                                }
                            }
                        }
                        MediaBoxType::HLS => {}
                        MediaBoxType::MP4 => {}
                    }
                }
            }
        }
        Ok(())
    }
}