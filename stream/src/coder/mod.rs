use common::bytes::Bytes;
use common::err::GlobalResult;

pub mod h264;


#[derive(Clone)]
pub enum FrameData {
    Video { timestamp: u32, data: Bytes },
    Audio { timestamp: u32, data: Bytes },
    MetaData { timestamp: u32, data: Bytes },
}

pub type HandleFrameDataFn = Box<dyn Fn(FrameData) -> GlobalResult<()> + Send + Sync>;