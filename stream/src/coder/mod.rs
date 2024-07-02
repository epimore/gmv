use common::bytes::Bytes;
use common::err::{GlobalResult};
use crate::coder::h264::H264;
use crate::general::mode::Coder;

pub mod h264;


#[derive(Clone)]
pub struct FrameData {
    pub pay_type: Coder,
    pub timestamp: u32,
    pub data: Bytes,
}

pub type HandleFrameDataFn = Box<dyn Fn(FrameData) -> GlobalResult<()> + Send + Sync>;

pub struct MediaCoder {
    pub h264: H264,
    // pub h265:H265,
    // pub aac:Aac,
}

impl MediaCoder {
    pub fn register_all(handle_fn: HandleFrameDataFn) -> Self {
        Self { h264: H264::init_avc(handle_fn) }
    }
}
