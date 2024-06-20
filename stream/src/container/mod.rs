use common::bytes::Bytes;
use common::err::GlobalResult;

pub mod rtp;
pub mod flv;

pub type HandleMuxerDataFn = Box<dyn Fn(Bytes) -> GlobalResult<()> + Send + Sync>;