use std::time::Duration;
use common::tokio;
use crate::io::IO;

mod io;
mod general;
pub mod data;
mod protocol;

#[tokio::main]
async fn main() {
    let tripe = common::init();
    let cfg = tripe.get_cfg().get(0).clone().expect("config file is invalid");
    let stream = general::mode::Stream::build(cfg);
    data::session::insert(1, "sid".to_string(), Duration::from_millis(*stream.get_timeout() as u64), None).expect("session init failed");
    stream.listen_input().await;
    // tokio::spawn(async move { stream.listen_input().await; });
    // loop {
    //     println!("can read start");
    //     data::buffer::Cache::readable(&1).await.expect("readable exception");
    //     println!("can read end");
    //     data::buffer::Cache::consume(&1).expect("consume exception");
    // }
}
