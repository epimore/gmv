use common::err::GlobalResult;
use common::tokio;

use crate::state::cache;

mod rtp_handler;
mod http_handler;
pub(crate) mod hook_handler;
mod rtcp_handler;

pub async fn run() -> GlobalResult<()> {
    let conf = cache::get_server_conf();
    let rtp_port = *(conf.get_rtp_port());
    let rtcp_port = *(conf.get_rtcp_port());
    let http_port = *(conf.get_http_port());
    tokio::spawn(http_handler::run(http_port));
    rtp_handler::run(rtp_port).await
}