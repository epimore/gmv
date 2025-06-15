use std::mem;
use std::sync::Arc;
use crate::media::format::muxer::MuxerEnum;
use common::bytes::Bytes;
use common::tokio::sync::{broadcast, mpsc};
use rsmpeg::ffi::{av_packet_unref, av_read_frame, AVPacket};
use crate::media::format::demuxer::{AvioResource, SendablePacket};

pub(super) mod demuxer;
mod muxer;
mod flv;
mod mp4;
mod hls;

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
    muxers: Vec<muxer::MuxerEnum>,
    demuxer_context: demuxer::DemuxerContext,
    event_tx: broadcast::Sender<RemuxerOutEvent>,
    event_rx: mpsc::Receiver<RemuxerInEvent>,
}

impl RemuxerLoop {
    pub fn read_loop(avio: Arc<AvioResource>, tx: broadcast::Sender<SendablePacket>) {
        let fmt_ctx = avio.fmt_ctx;
        unsafe {
            let mut pkt = std::mem::zeroed::<AVPacket>();
            loop {
                if av_read_frame(fmt_ctx, &mut pkt) < 0 {
                    break;
                }
                let cloned = SendablePacket::from_avpacket(&pkt);
                let _ = tx.send(cloned);
                av_packet_unref(&mut pkt);
            }
        }
    }
    
    pub fn new(init_muxer: muxer::MuxerEnum,
               demuxer_context: demuxer::DemuxerContext,
               event_tx: broadcast::Sender<RemuxerOutEvent>,
               event_rx: mpsc::Receiver<RemuxerInEvent>) -> Self {
        Self {
            muxers: vec![init_muxer],
            demuxer_context,
            event_tx,
            event_rx,
        }
    }

    fn add_muxer(&mut self, box_type: MediaBoxType, call_new_muxer: fn() -> MuxerEnum) -> bool {
        for muxer in &self.muxers {
            match (box_type, muxer) {
                (MediaBoxType::FLV, muxer::MuxerEnum::Flv(_)) => return false,
                (MediaBoxType::HLS, muxer::MuxerEnum::Hls(_)) => return false,
                (MediaBoxType::MP4, muxer::MuxerEnum::Mp4(_)) => return false,
                _ => {}
            }
        }
        self.muxers.push(call_new_muxer());
        true
    }

    pub fn event_in_hook(&mut self) {
        if let Ok(event_in) = self.event_rx.try_recv() {
            match event_in {
                RemuxerInEvent::OPEN(media_box) => {
                    match media_box {
                        MediaBoxInner::FLV(flv_tx) => {
                            flv::FlvMuxer::new(flv_tx, &self.demuxer_context);
                        }
                        MediaBoxInner::HLS => {}
                        MediaBoxInner::MP4 => {}
                    }
                }
                RemuxerInEvent::CLOSE(mbt) => {
                    self.muxers.retain(|muxer| match (mbt, muxer) {
                        (MediaBoxType::FLV, muxer::MuxerEnum::Flv(_)) => false,
                        (MediaBoxType::HLS, muxer::MuxerEnum::Hls(_)) => false,
                        (MediaBoxType::MP4, muxer::MuxerEnum::Mp4(_)) => false,
                        _ => true,  // 其他情况保留
                    })
                }
                RemuxerInEvent::HEADER(header) => {
                    match header {
                        MediaBoxType::FLV => {
                            for muxer in &self.muxers {
                                if let muxer::MuxerEnum::Flv(flv) = muxer {
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
    }
}