use rsmpeg::ffi::AVPacket;
use crate::media::format::muxer::MuxerSink;

pub struct Mp4Muxer {}

impl MuxerSink for Mp4Muxer {
    fn write_packet(&self, pkt: &AVPacket) {
        unimplemented!()
    }
}