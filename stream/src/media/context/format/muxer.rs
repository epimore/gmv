use common::exception::GlobalResult;
use rsmpeg::ffi::AVPacket;
use crate::media::msg::format::demuxer::DemuxerContext;
use crate::media::msg::format::flv::FlvMuxer;
use crate::media::msg::format::hls::HlsMuxer;
use crate::media::msg::format::mp4::Mp4Muxer;

// pub enum MuxerEnum {
//     Flv(FlvMuxer),
//     Mp4(Mp4Muxer),
//     Hls(HlsMuxer),
// }

pub trait MuxerSink<T> {
    fn init_muxer(dc: &DemuxerContext, t: T) -> GlobalResult<Self>
    where
        Self: Sized;
    fn write_packet(&mut self, pkt: &AVPacket);
}

// pub trait MuxerBuilder: Send + Sync {
//     fn build(&self, ctx: &DemuxerContext) -> GlobalResult<MuxerEnum>;
// }