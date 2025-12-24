pub mod mp4;
pub mod ts;

use base::bytes::Bytes;

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
