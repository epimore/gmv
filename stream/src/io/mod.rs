use std::thread;

use common::err::{GlobalResult};
use common::tokio;
use common::tokio::sync::mpsc;

use crate::state::cache;
use crate::trans;

mod rtp_handler;
mod http_handler;
pub(crate) mod hook_handler;
mod rtcp_handler;

pub async fn run() -> GlobalResult<()> {
    let conf = cache::get_server_conf();
    let node_name = conf.get_name();
    let rtp_port = *(conf.get_rtp_port());
    let http_port = *(conf.get_http_port());
    let (tx, rx) = mpsc::channel(100);
    tokio::spawn(async move{ trans::run(rx).await; });
    thread::spawn(move || {
        tokio::runtime::Runtime::new().map(|rt| {
            rt.block_on(rtp_handler::run(rtp_port));
        }).expect("RTP:IO 运行时创建异常；err ={}");
    });
    http_handler::run(node_name, http_port, tx).await
}
