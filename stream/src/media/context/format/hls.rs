use common::exception::GlobalResult;
use rsmpeg::ffi::AVPacket;
use crate::media::msg::format::demuxer::DemuxerContext;
use crate::media::msg::format::muxer::{MuxerEnum, MuxerSink};


pub struct HlsMuxer {}
impl MuxerSink<bool> for HlsMuxer {
    fn write_packet(&mut self, pkt: &AVPacket) {
        unimplemented!()
    }

    fn init_muxer(dc: &DemuxerContext, t: bool) -> GlobalResult<MuxerEnum> {
        unimplemented!()
    }
}