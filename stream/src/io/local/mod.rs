mod mp4;
mod ts;

use base::bytes::Bytes;
use base::exception::GlobalResultExt;

pub struct StreamData {
    pub data: Bytes,
    pub end: bool,
}

pub struct LocalStream {
    pub path: String,
    pub ssrc: u32,
    pub data: Bytes,
}
pub async fn loop_store(ssrc: u32) {}
