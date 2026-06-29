pub mod control_rpc;
pub mod event_forwarder;
pub mod node_expirer;
pub mod node_rpc;
pub mod web;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Ready,
    Stopping,
}

use std::path::PathBuf;

use axum::Router;

use crate::api::v2::http::{self, HttpState};

#[cfg(not(feature = "embed-ui"))]
pub fn application_router(state: HttpState, dist_dir: impl Into<PathBuf>) -> Router {
    http::router(state).merge(crate::ui::dist_router(dist_dir))
}

#[cfg(feature = "embed-ui")]
pub fn application_router(state: HttpState, _dist_dir: impl Into<PathBuf>) -> Router {
    http::router(state).merge(crate::ui::embedded_router())
}
