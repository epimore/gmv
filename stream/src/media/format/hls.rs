use rsmpeg::ffi::AVPacket;
use crate::media::format::muxer::MuxerSink;

pub struct HlsMuxer {}
impl MuxerSink for HlsMuxer {
    fn write_packet(&self, pkt: &AVPacket) {
        unimplemented!()
    }
}