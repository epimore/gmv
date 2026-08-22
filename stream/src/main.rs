#![allow(warnings)]
use base::daemon;

mod app;
pub mod general;
pub mod guard_integration;
pub mod io;
mod media;
pub mod state;

fn main() {
    daemon::run::<app::App, _>();
}
