use crate::media::context::format::FmtMuxer;
use crate::media::context::format::dashmp4::DashCmafMp4Context;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::flv::{FlvContext, FlvSupperCtx};
use crate::media::context::format::fmp4::CmafFmp4Context;
use crate::media::context::format::h265flv::H265FlvContext;
use crate::media::context::format::hlsfmp4::HlsFmp4Context;
use crate::media::context::format::muxer::{MuxerContext, MuxerEnum};
use crate::state::layer::muxer_layer::{
    CMafLayer, FlvLayer, HlsTsLayer, Mp4Layer, RtpEncLayer, RtpFrameLayer, RtpPsLayer, TsLayer,
};
use base::log::warn;
use rsmpeg::ffi::AVCodecID_AV_CODEC_ID_HEVC;

pub enum MuxerEvent {
    Open(MuxerKind),
    Close(MuxerEnum),
}
impl MuxerEvent {
    pub fn handle_event(self, muxer_context: &mut MuxerContext, demuxer_context: &DemuxerContext) {
        match self {
            MuxerEvent::Open(open) => match open {
                MuxerKind::Flv(flv) => unsafe {
                    let in_fmt_ctx = demuxer_context.avio.fmt_ctx;
                    if (*in_fmt_ctx).video_codec_id == AVCodecID_AV_CODEC_ID_HEVC {
                        let _ = H265FlvContext::init_context(demuxer_context, flv.tx).map(
                            |flv_context| {
                                muxer_context.flv = Some(FlvSupperCtx::H265FlvCtx(flv_context));
                            },
                        );
                    } else {
                        let _ =
                            FlvContext::init_context(demuxer_context, flv.tx).map(|flv_context| {
                                muxer_context.flv = Some(FlvSupperCtx::FlvCtx(flv_context));
                            });
                    }
                },
                MuxerKind::Ts(_) => {
                    warn!("stream muxer event ignored unsupported ts output");
                }
                MuxerKind::Mp4(_) => {
                    warn!("stream muxer event ignored unsupported mp4 output");
                }
                MuxerKind::FMp4(fmp4) => {
                    let _ = CmafFmp4Context::init_context(demuxer_context, fmp4.tx).map(|ctx| {
                        muxer_context.fmp4 = Some(ctx);
                    });
                }
                MuxerKind::HlsTs(_) => {
                    warn!("stream muxer event ignored unsupported hls-ts output");
                }
                MuxerKind::RtpFrame(_) => {
                    warn!("stream muxer event ignored unsupported rtp-frame output");
                }
                MuxerKind::RtpPs(_) => {
                    warn!("stream muxer event ignored unsupported rtp-ps output");
                }
                MuxerKind::RtpEnc(_) => {
                    warn!("stream muxer event ignored unsupported rtp-enc output");
                }
                MuxerKind::DashMp4(dash_mp4) => {
                    let _ =
                        DashCmafMp4Context::init_context(demuxer_context, dash_mp4.tx).map(|ctx| {
                            muxer_context.dash_mp4 = Some(ctx);
                        });
                }
                MuxerKind::HlsMp4(hls_mp4) => {
                    let _ = HlsFmp4Context::init_context(demuxer_context, hls_mp4.tx).map(|ctx| {
                        muxer_context.hls_mp4 = Some(ctx);
                    });
                }
            },
            // cache缓存的media layer；在发布关闭事件时做出判断-关闭是否muxer/filter等关联输出是否为空，为空则直接释放对应的ssrc资源,不会造成空转
            MuxerEvent::Close(muxer_enum) => match muxer_enum {
                MuxerEnum::Flv => muxer_context.flv = None,
                MuxerEnum::Mp4 => {
                    if let Some(mp4_ctx) = &mut muxer_context.mp4 {
                        mp4_ctx.flush();
                        muxer_context.mp4 = None;
                    }
                }
                MuxerEnum::Ts => muxer_context.ts = None,
                MuxerEnum::FMp4 => muxer_context.fmp4 = None,
                MuxerEnum::HlsTs => muxer_context.hls_ts = None,
                MuxerEnum::RtpFrame => muxer_context.rtp_frame = None,
                MuxerEnum::RtpPs => muxer_context.rtp_ps = None,
                MuxerEnum::RtpEnc => muxer_context.rtp_enc = None,
                MuxerEnum::DashMp4 => muxer_context.dash_mp4 = None,
                MuxerEnum::HlsMp4 => muxer_context.hls_mp4 = None,
            },
        }
    }
}

pub enum MuxerKind {
    Flv(FlvLayer),
    Mp4(Mp4Layer),
    DashMp4(CMafLayer),
    Ts(TsLayer),
    FMp4(CMafLayer),
    HlsMp4(CMafLayer),
    HlsTs(HlsTsLayer),
    RtpFrame(RtpFrameLayer),
    RtpPs(RtpPsLayer),
    RtpEnc(RtpEncLayer),
}
