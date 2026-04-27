#![allow(warnings)]
use base::daemon;
use crate::app::AppInfo;
pub mod storage;
pub mod gb;
pub mod state;
mod service;
pub mod utils;
mod app;
mod http;
pub mod register;

fn main() {
    daemon::run::<AppInfo, _>();
}