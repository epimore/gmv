use shared::info::format::MuxerType;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::flv::FlvContext;
use crate::media::context::format::muxer::MuxerContext;
use crate::state::layer::muxer_layer::{FlvLayer, FrameLayer, Mp4Layer, RtpEncLayer, RtpFrameLayer, RtpPsLayer, TsLayer};

pub enum MuxerEvent {
    Open(OpenMuxer),
    Close(CloseMuxer),
}
impl MuxerEvent {
    pub fn handle_event(self, muxer_context: &mut MuxerContext, demuxer_context: &DemuxerContext) {
        match self {
            MuxerEvent::Open(open) => {
                match open {
                    OpenMuxer::Flv(flv) => {
                        let _ = FlvContext::init_context(demuxer_context, flv.tx).map(|flv_context| {
                            muxer_context.flv = Some(flv_context);
                        });
                    }
                    OpenMuxer::Ts(ts) => { unimplemented!() }
                    OpenMuxer::Mp4(mp4) => { unimplemented!() }
                    OpenMuxer::RtpFrame(rtp_frame) => { unimplemented!() }
                    OpenMuxer::RtpPs(rtp_ps) => { unimplemented!() }
                    OpenMuxer::RtpEnc(rtp_enc) => { unimplemented!() }
                    OpenMuxer::Frame(frame) => { unimplemented!() }
                }
            }
            // cache缓存的media layer；在发布关闭事件时做出判断-关闭是否muxer/filter等关联输出是否为空，为空则直接释放对应的ssrc资源,不会造成空转
            MuxerEvent::Close(close) => {
                match close {
                    CloseMuxer::Flv => muxer_context.flv = None,
                    CloseMuxer::Ts => muxer_context.ts = None,
                    CloseMuxer::Mp4 => muxer_context.mp4 = None,
                    CloseMuxer::RtpFrame => muxer_context.rtp_frame = None,
                    CloseMuxer::RtpPs => muxer_context.rtp_ps = None,
                    CloseMuxer::RtpEnc => muxer_context.rtp_enc = None,
                    CloseMuxer::Frame => muxer_context.frame = None,
                }
            }
        }
    }
}

pub enum OpenMuxer {
    Flv(FlvLayer),
    Ts(TsLayer),
    Mp4(Mp4Layer),
    RtpFrame(RtpFrameLayer),
    RtpPs(RtpPsLayer),
    RtpEnc(RtpEncLayer),
    Frame(FrameLayer),
}

pub enum CloseMuxer {
    Flv,
    Ts,
    Mp4,
    RtpFrame,
    RtpPs,
    RtpEnc,
    Frame,
}
impl CloseMuxer {
    pub fn from_muxer_type(tp: &MuxerType) -> Option<Self> {
        match tp {
            MuxerType::None => { None }
            MuxerType::Flv => { Some(Self::Flv) }
            MuxerType::Mp4 => { Some(Self::Mp4) }
            MuxerType::Ts => { Some(Self::Ts) }
            MuxerType::RtpFrame => { Some(Self::RtpFrame) }
            MuxerType::RtpPs => { Some(Self::RtpPs) }
            MuxerType::RtpEnc => { Some(Self::RtpEnc) }
            MuxerType::Frame => { Some(Self::Frame) }
        }
    }
}