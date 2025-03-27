#![allow(warnings)]
use common::daemon;
use crate::app::AppInfo;
pub mod storage;
pub mod gb;
pub mod general;
mod web;
mod service;
mod utils;
mod app;

fn main() {
    daemon::run::<AppInfo, _>();
}
