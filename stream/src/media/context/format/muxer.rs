use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::flv::FlvContext;
use crate::media::context::format::frame::FrameContext;
use crate::media::context::format::mp4::Mp4Context;
use crate::media::context::format::rtp::{RtpEncContext, RtpFrameContext, RtpPsContext};
use crate::media::context::format::ts::TsContext;
use crate::state::layer::muxer_layer::MuxerLayer;

#[derive(Default)]
pub struct MuxerContext {
    pub flv: Option<FlvContext>,
    pub mp4: Option<Mp4Context>,
    pub ts: Option<TsContext>,
    pub frame: Option<FrameContext>,
    pub rtp_ps: Option<RtpPsContext>,
    pub rtp_enc: Option<RtpEncContext>,
    pub rtp_frame: Option<RtpFrameContext>,

}
impl MuxerContext {
    pub fn init(demuxer_context: &DemuxerContext,muxer: MuxerLayer) -> MuxerContext {
        let mut context = MuxerContext::default();
        if let Some(flv_layer) = &muxer.flv {
            let _ = FlvContext::init_context(demuxer_context, flv_layer.tx.clone()).map(|flv_context| {
                context.flv = Some(flv_context);
            });
        }
        if let Some(mp4_layer) = &muxer.mp4 { unimplemented!() }
        if let Some(ts_layer) = &muxer.ts { unimplemented!() }
        if let Some(rtp_frame_layer) = &muxer.rtp_frame { unimplemented!() }
        if let Some(rtp_ps_layer) = &muxer.rtp_ps { unimplemented!() }
        if let Some(rtp_enc_layer) = &muxer.rtp_enc { unimplemented!() }
        if let Some(frame_layer) = &muxer.frame { unimplemented!() }
        context
    }
}