use common::bytes::Bytes;
use common::exception::GlobalResult;
use common::serde::{Deserialize, Serialize};

pub mod rtp;
pub mod flv;
pub mod ps;

///rtp /flv等容器封装h264时,需剔除0000000001/000001开始符
pub type HandleMuxerDataFn = Box<dyn Fn(Bytes) -> GlobalResult<()> + Send + Sync>;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum PlayType {
    Flv,
    Hls,
}