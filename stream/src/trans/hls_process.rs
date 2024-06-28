use common::err::GlobalResult;
use common::tokio::sync::broadcast;
use crate::coder::FrameData;

pub async fn run(ssrc: u32, mut rx: broadcast::Receiver<FrameData>) -> GlobalResult<()> {
    unimplemented!()
}