use common::{logger, tokio};
use common::dbx::mysqlx;
use crate::general::http;

// #![allow(warnings)]
pub mod storage;
pub mod gb;
pub mod general;
mod web;
mod service;
mod utils;

#[tokio::main]
async fn main() {
    banner();
    logger::Logger::init();
    mysqlx::init_conn_pool();
    tokio::spawn(async move {
        http::Http::init_http_server().await;
    });
    gb::init_gb_server().await;
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
