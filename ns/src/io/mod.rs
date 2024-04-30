use common::err::GlobalResult;
use common::tokio;

use crate::state::cache;

mod rtp_stream;
mod http_api;
pub(crate) mod event_hook;

pub async fn run() -> GlobalResult<()> {
    let conf = cache::get_server_conf();
    let rtp_port = *(conf.get_rtp_port());
    let rtcp_port = *(conf.get_rtcp_port());
    let http_port = *(conf.get_http_port());
    tokio::spawn(http_api::run(http_port));
    rtp_stream::run(rtp_port).await
}