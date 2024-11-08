// #![allow(warnings)]
use common::exception::TransError;
use common::log::error;
use common::{logger, tokio};

mod io;
pub mod general;
pub mod state;
mod biz;
mod trans;
pub mod coder;
pub mod container;

#[tokio::main]
async fn main() {
    logger::Logger::init();
    let _ = io::run().await.hand_log(|msg| error!("{msg}"));
    //todo ctrl_c
}
