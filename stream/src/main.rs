#![allow(warnings)]
use base::{daemon};

pub mod io;
pub mod general;
pub mod state;
mod app;
mod media;

fn main() {
    daemon::run::<app::App, _>();
}
