use rsmpeg::ffi::AVPacket;
use crate::media::format::flv::FlvMuxer;
use crate::media::format::hls::HlsMuxer;
use crate::media::format::mp4::Mp4Muxer;

pub(crate) enum MuxerEnum {
    Flv(FlvMuxer),
    Mp4(Mp4Muxer),
    Hls(HlsMuxer),
}

pub trait MuxerSink {
    fn write_packet(&self, pkt: &AVPacket);
}

