 // #![allow(warnings)]
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
    banner();
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

fn banner() {
    let br = r#"
            ___   __  __  __   __    _      ___     ___     ___     ___     ___     ___    _  _
    o O O  / __| |  \/  | \ \ / /   (_)    / __|   | __|   / __|   / __|   |_ _|   / _ \  | \| |
   o      | (_ | | |\/| |  \ V /     _     \__ \   | _|    \__ \   \__ \    | |   | (_) | | .` |
  o0__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   |___|   |___/   |___/   |___|   \___/  |_|\_|
 {======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""E""|_|""S""|_|""S""|_|""I""|_|""O""|_|""N""|
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
"#;
    println!("{}", br);
}
