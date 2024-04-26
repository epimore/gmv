use common::tokio;
use crate::general::mode::HttpStream;

pub mod io;
pub mod general;
pub mod state;

#[tokio::main]
async fn main() {
    let mut stream = HttpStream::default();
    stream.set_port(12233u16);
    io::output::listen_stream(stream).await.unwrap();

    // let tripe = common::init();
    // let cfg = tripe.get_cfg().get(0).clone().expect("config file is invalid");
    // let stream = general::mode::Stream::build(cfg);
    // println!("Hello, world!");
}
