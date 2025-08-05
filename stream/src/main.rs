#![allow(warnings)]
use common::{daemon};

pub mod io;
pub mod general;
pub mod state;
mod app;
mod media;

fn main() {
    daemon::run::<app::App, _>();
}
