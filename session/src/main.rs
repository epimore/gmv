#![allow(warnings)]
use common::daemon;
use crate::app::AppInfo;
pub mod storage;
pub mod gb;
pub mod general;
mod service;
pub mod utils;
mod app;
mod http;

fn main() {
    daemon::run::<AppInfo, _>();
}
/*
todo 
call 统一封装 
1.call ssrc listen
2.call rtp map
3.统一db查询状态
 
*/