// #![allow(warnings)]
use common::{daemon};

pub mod io;
pub mod general;
pub mod state;
mod biz;
mod trans;
pub mod coder;
pub mod container;
mod app;
mod comm;

fn main() {
    daemon::run::<app::App, _>();
}
