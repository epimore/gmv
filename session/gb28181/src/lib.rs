#![allow(warnings)]

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

#[cfg(test)]
mod normal_flow_tests;

pub fn run() {
    daemon::run::<AppInfo, _>();
}
