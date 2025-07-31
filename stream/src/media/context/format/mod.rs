use crate::media::msg::format::muxer::{MuxerEnum, MuxerSink};
use common::bytes::Bytes;
use common::exception::{GlobalResult};
use common::log::warn;
use common::tokio::sync::{broadcast, mpsc, oneshot};
use rsmpeg::ffi::{AVPacket};
use common::bus::mpsc::TypedReceiver;
use crate::media::msg::format::demuxer::{DemuxerContext};
use crate::media::rtp;
use crate::state::msg::SdpMsg;

pub(in crate::media) mod demuxer;
pub mod muxer;
pub mod flv;
pub mod mp4;
pub mod hls;
pub struct RemuxerContext {
    pub sdp_msg: SdpMsg,
    pub rtp_buffer: rtp::RtpPacketBuffer,
    pub event_rx: TypedReceiver<RemuxerInEvent>,
}
impl RemuxerContext {
    pub fn start_remuxer<T>(self, t: T, init_muxer_callback: fn(&DemuxerContext, T) -> GlobalResult<MuxerEnum>) -> GlobalResult<()>
    {
        let demuxer_context = DemuxerContext::start_demuxer(&self.sdp_msg, self.rtp_buffer)?;
        let init_muxer_enum = init_muxer_callback(&demuxer_context, t)?;
        let mut remuxer_loop = RemuxerLoop::new(init_muxer_enum, demuxer_context, self.event_rx);
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

#[derive(Clone, Copy)]
pub enum CloseMediaBox {
    FLV,
    HLS,
    MP4,
}
pub enum OpenMediaBox {
    //is_idr,packet
    FLV(broadcast::Sender<(bool, Bytes)>),
    HLS,
    MP4,
}
pub enum HeaderMediaBox {
    FLV(oneshot::Sender<Bytes>),
    HLS,
    MP4,
}

pub enum RemuxerInEvent {
    OPEN(OpenMediaBox),
    CLOSE(CloseMediaBox),
    HEADER(HeaderMediaBox),
}

pub struct RemuxerLoop {
    muxers: Vec<MuxerEnum>,
    demuxer_context: DemuxerContext,
    event_rx: TypedReceiver<RemuxerInEvent>,
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
                pkt.stream_index == 0;

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
            Ok(())
        }
    }

    // fn new(init_muxer: MuxerEnum,
    //        demuxer_context: DemuxerContext,
    //        event_rx: TypedReceiver<RemuxerInEvent>) -> Self {
    //     Self {
    //         muxers: vec![init_muxer],
    //         demuxer_context,
    //         event_rx,
    //     }
    // }
    // 
    // fn add_muxer<F: FnOnce() -> GlobalResult<MuxerEnum>>(muxers: &mut Vec<MuxerEnum>, box_type: MediaBoxType, call_new_muxer: F) -> bool {
    //     for muxer in muxers.iter() {
    //         match (box_type, muxer) {
    //             (MediaBoxType::FLV, MuxerEnum::Flv(_)) => return false,
    //             (MediaBoxType::HLS, MuxerEnum::Hls(_)) => return false,
    //             (MediaBoxType::MP4, MuxerEnum::Mp4(_)) => return false,
    //             _ => {}
    //         }
    //     }
    //     if let Ok(call_new_muxer) = call_new_muxer() {
    //         muxers.push(call_new_muxer);
    //         true
    //     } else { false }
    // }
    // 
    // fn event_in_hook(&mut self) -> GlobalResult<()> {
    //     if let Ok(event_in) = self.event_rx.try_recv() {
    //         match event_in {
    //             RemuxerInEvent::OPEN(media_box) => {
    //                 match media_box {
    //                     OpenMediaBox::FLV(flv_tx) => {
    //                         let flv_muxer_enum = || {
    //                             let flv_muxer = flv::FlvMuxer::init_muxer(&self.demuxer_context, flv_tx)?;
    //                             Ok(flv_muxer)
    //                         };
    //                         if !Self::add_muxer(&mut self.muxers, MediaBoxType::FLV, flv_muxer_enum) {
    //                             warn!("flv muxer already exist");
    //                         }
    //                     }
    //                     OpenMediaBox::HLS => {}
    //                     OpenMediaBox::MP4 => {}
    //                 }
    //             }
    //             RemuxerInEvent::CLOSE(mbt) => {
    //                 self.muxers.retain(|muxer| match (mbt, muxer) {
    //                     (CloseMediaBox::FLV, MuxerEnum::Flv(_)) => false,
    //                     (CloseMediaBox::HLS, MuxerEnum::Hls(_)) => false,
    //                     (CloseMediaBox::MP4, MuxerEnum::Mp4(_)) => false,
    //                     _ => true,  // 其他情况保留
    //                 })
    //             }
    //             RemuxerInEvent::HEADER(header) => {
    //                 match header {
    //                     HeaderMediaBox::FLV(tx) => {
    //                         for muxer in &self.muxers {
    //                             if let MuxerEnum::Flv(flv) = muxer {
    //                                 let _ = tx.send(flv.get_header()).map_err(|_| warn!("send flv header: channel closed"));
    //                                 break;
    //                             }
    //                         }
    //                     }
    //                     HeaderMediaBox::HLS => {}
    //                     HeaderMediaBox::MP4 => {}
    //                 }
    //             }
    //         }
    //     }
    //     Ok(())
    // }
}