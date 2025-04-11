use common::bytes::Bytes;
use common::exception::GlobalResult;
use common::serde::{Deserialize, Serialize};

pub mod rtp;
pub mod flv;
pub mod ps;
pub mod hls;
pub mod mp4;

///rtp /flv等容器封装h264时,需剔除0000000001/000001开始符
pub type HandleMuxerDataFn = Box<dyn Fn(Bytes) -> GlobalResult<()> + Send + Sync>;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum PlayType {
    Flv,
    Hls,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum ContainerType {
    Flv,
    Mp4,
    // Hls,
}

pub trait PacketWriter {
    fn packet(&mut self, vec_frame: &mut Vec<Bytes>, timestamp: u32);
}