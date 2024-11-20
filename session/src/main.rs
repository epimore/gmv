
use common::{daemon};

use crate::app::AppInfo;

// #![allow(warnings)]
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
