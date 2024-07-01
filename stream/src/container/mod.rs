use common::bytes::Bytes;
use common::err::GlobalResult;
use crate::container::flv::VideoTagDataBuffer;
use crate::general::mode::Coder;

pub mod rtp;
pub mod flv;

///rtp /flv等容器封装h264时,需剔除0000000001/000001开始符
pub type HandleMuxerDataFn = Box<dyn Fn(Bytes) -> GlobalResult<()> + Send + Sync>;