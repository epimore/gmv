pub mod storage;
pub mod gb;
pub mod general;
mod web;
mod service;
mod utils;

use common::log::error;
use common::err::TransError;
use common::tokio;


#[tokio::main]
async fn main() {
    let tripe = common::init();
    let cfg = tripe.get_cfg().get(0).clone().expect("config file is invalid");
    idb::init_mysql(cfg);
    let yaml = cfg.clone();
    tokio::spawn(async move {
        let http = general::http::Http::build(&yaml);
        http.init_web_server((web::api::RestApi, web::hook::HookApi)).await
    });
    let conf = general::SessionConf::get_session_conf(cfg);
    let _ = gb::gb_run(&conf).await.hand_log(|msg| error!("GB RUN FAILED <<< [{msg}]"));
}
