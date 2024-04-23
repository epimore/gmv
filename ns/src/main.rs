use common::tokio;

mod io;
mod general;
pub mod state;

#[tokio::main]
async fn main() {
    let tripe = common::init();
    let cfg = tripe.get_cfg().get(0).clone().expect("config file is invalid");
    let stream = general::mode::Stream::build(cfg);
    println!("Hello, world!");
}
