use crossbeam_channel::Sender;

use common::bytes::Bytes;
use common::exception::GlobalResult;

use crate::coder::h264::H264;
use crate::container::ps::Ps;
use crate::general::mode::Coder;

pub mod h264;


#[derive(Clone)]
pub struct FrameData {
    pub pay_type: Coder,
    pub timestamp: u32,
    pub data: Bytes,
}

pub type HandleFrameDataFn = Box<dyn Fn(FrameData) -> GlobalResult<()> + Send + Sync>;

pub struct MediaInfo {
    pub h264: H264,
    pub ps: Ps,
    // pub h265:H265,
    // pub aac:Aac,
}

impl MediaInfo {
    pub fn register_all(flv_tx: Option<Sender<FrameData>>, hls_tx: Option<Sender<FrameData>>) -> Self {
        Self { h264: H264::init_avc(flv_tx.clone(), hls_tx.clone()), ps: Ps::init(flv_tx.clone(), hls_tx.clone()) }
    }
}

pub trait HandleFrame {
    fn next_step(&self, frame_data: FrameData) -> GlobalResult<()>;
}