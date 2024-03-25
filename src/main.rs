pub mod storage;
pub mod gb;
mod the_common;
use common::tokio;


#[tokio::main]
async fn main() {
    let tripe = common::init();
    idb::init_mysql(tripe.get_cfg());
    let conf = the_common::SessionConf::get_session_conf(tripe.get_cfg().clone().get(0).expect("config file is invalid"));
    let _ = gb::gb_run(&conf).await;
    println!("Hello, world!");
}
