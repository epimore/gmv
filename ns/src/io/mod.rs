use common::err::{GlobalResult, TransError};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc;

use crate::state::cache;

mod rtp_handler;
mod http_handler;
pub(crate) mod hook_handler;
mod rtcp_handler;
mod converter;

pub async fn run() -> GlobalResult<()> {
    let conf = cache::get_server_conf();
    let rtp_port = *(conf.get_rtp_port());
    let rtcp_port = *(conf.get_rtcp_port());
    let http_port = *(conf.get_http_port());
    let (tx, rx) = mpsc::channel(100);
    tokio::spawn(async move { let _ = converter::run(rx).await; });
    let tx_cl = tx.clone();
    tokio::spawn(async move { let _ = http_handler::run(http_port, tx_cl).await.hand_err(|msg| error!("{msg}")); });
    rtp_handler::run(rtp_port).await
}
