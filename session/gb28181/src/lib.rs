#![allow(warnings)]

#[cfg(not(any(feature = "db-mysql", feature = "db-sqlite")))]
compile_error!("enable at least one database feature: db-mysql or db-sqlite");

use crate::app::AppInfo;
use base::daemon;

mod app;
pub mod gb;
pub mod guard_integration;
mod http;
pub mod register;
mod service;
pub mod state;
pub mod storage;
pub mod utils;

pub fn run() {
    daemon::run::<AppInfo, _>();
}
