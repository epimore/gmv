#![allow(warnings)]
use crate::app::AppInfo;
use base::daemon;
mod app;
pub mod gb;
mod http;
pub mod register;
mod service;
pub mod state;
pub mod storage;
pub mod utils;

fn main() {
    daemon::run::<AppInfo, _>();
}
