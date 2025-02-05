use common::bytes::Bytes;
use common::exception::GlobalResult;
use rtp::packet::Packet;

use crate::general::mode::Coder;

pub mod h264;


pub enum VideoCodec {
    H264,
    H265,
}

pub enum AudioCodec {
    G711,
    // SVAC_A,
    // G723_1,
    // G729,
    // G722_1,
    // AAC,
}

#[derive(Clone)]
pub struct FrameData {
    pub pay_type: Coder,
    pub timestamp: u32,
    pub data: Bytes,
}

pub type HandleFrameDataFn = Box<dyn Fn(FrameData) -> GlobalResult<()> + Send + Sync>;

#[derive(Default)]
pub struct CodecPayload {
    //codec,data,timestamp
    pub video_payload: (Option<VideoCodec>, Vec<Bytes>, u32),
    pub audio_payload: (Option<AudioCodec>, Vec<Bytes>, u32),
    // pub other_payload:(Vec<Bytes>,u32),#字幕/私有信息...
}


pub trait ToFrame {
    fn parse(&mut self, pkt: Packet, codec_payload: &mut CodecPayload) -> GlobalResult<()>;
}