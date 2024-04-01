use std::time::Duration;
use common::log::info;
use common::tokio;
use crate::io::IO;

mod io;
mod general;
pub mod data;
mod protocol;
//ffmpeg -re -i E:\book\mv\st\bigbuckbunnynoB_480x272.h264 -vcodec copy -f rtp rtp://172.18.38.186:18547>test_rtp_h264.sdp
#[tokio::main]
async fn main() {
    let tripe = common::init();
    let cfg = tripe.get_cfg().get(0).clone().expect("config file is invalid");
    let stream = general::mode::Stream::build(cfg);
    data::session::insert(1, "sid".to_string(), Duration::from_millis(*stream.get_timeout() as u64), None).expect("session init failed");
    tokio::spawn(async move { stream.listen_input().await; });
    loop {
        data::buffer::Cache::readable(&1).await.expect("readable exception");
        data::buffer::Cache::consume(&1).expect("consume exception");
    }
}
