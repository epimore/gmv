use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::flv::FlvContext;
use crate::media::context::format::muxer::MuxerContext;
use crate::state::layer::muxer_layer::{CMafLayer, FlvLayer, HlsTsLayer, Mp4Layer, RtpEncLayer, RtpFrameLayer, RtpPsLayer, TsLayer};
use shared::info::muxer::MuxerEnum;

pub enum MuxerEvent {
    Open(MuxerKind),
    Close(MuxerEnum),
}
impl MuxerEvent {
    pub fn handle_event(self, muxer_context: &mut MuxerContext, demuxer_context: &DemuxerContext) {
        match self {
            MuxerEvent::Open(open) => match open {
                MuxerKind::Flv(flv) => {
                    let _ = FlvContext::init_context(demuxer_context, flv.tx).map(|flv_context| {
                        muxer_context.flv = Some(flv_context);
                    });
                }
                MuxerKind::Ts(ts) => {
                    unimplemented!()
                }
                MuxerKind::Mp4(mp4) => {
                    unimplemented!()
                }
                MuxerKind::FMp4(fmp4) => {
                    unimplemented!()
                }
                MuxerKind::HlsTs(hls_ts) => {
                    unimplemented!()
                }
                MuxerKind::RtpFrame(rtp_frame) => {
                    unimplemented!()
                }
                MuxerKind::RtpPs(rtp_ps) => {
                    unimplemented!()
                }
                MuxerKind::RtpEnc(rtp_enc) => {
                    unimplemented!()
                }
            },
            // cache缓存的media layer；在发布关闭事件时做出判断-关闭是否muxer/filter等关联输出是否为空，为空则直接释放对应的ssrc资源,不会造成空转
            MuxerEvent::Close(muxer_enum) => match muxer_enum {
                MuxerEnum::Flv => muxer_context.flv = None,
                MuxerEnum::Mp4 => muxer_context.mp4 = None,
                MuxerEnum::Ts => muxer_context.ts = None,
                MuxerEnum::FMp4 => muxer_context.fmp4 = None,
                MuxerEnum::HlsTs => muxer_context.hls_ts = None,
                MuxerEnum::RtpFrame => muxer_context.rtp_frame = None,
                MuxerEnum::RtpPs => muxer_context.rtp_ps = None,
                MuxerEnum::RtpEnc => muxer_context.rtp_enc = None,
            },
        }
    }
}

pub enum MuxerKind {
    Flv(FlvLayer),
    Mp4(Mp4Layer),
    Ts(TsLayer),
    FMp4(CMafLayer),
    HlsTs(HlsTsLayer),
    RtpFrame(RtpFrameLayer),
    RtpPs(RtpPsLayer),
    RtpEnc(RtpEncLayer),
}