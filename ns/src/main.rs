use common::err::TransError;
use common::log::error;
use common::tokio;

mod io;
pub mod general;
pub mod state;
mod biz;
mod converter;

#[tokio::main]
async fn main() {
    let _ = io::run().await.hand_err(|msg| error!("{msg}"));
    //todo ctrl_c
}
