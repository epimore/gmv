#![allow(warnings)]
use common::err::TransError;
use common::log::error;
use common::tokio;

mod io;
pub mod general;
pub mod state;
mod biz;
mod trans;
pub mod coder;
pub mod container;

#[tokio::main]
async fn main() {
    let _ = io::run().await.hand_log(|msg| error!("{msg}"));
    //todo ctrl_c
}
