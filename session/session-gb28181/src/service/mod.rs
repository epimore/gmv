pub mod api_serv;
pub mod dialog_recovery;
pub mod edge_serv;
pub mod hook_serv;
pub mod stream_close;
mod talk;
pub mod talk_close;

pub const EXPIRES: u64 = 8;
pub const SNAPSHOT_IDLE_EXPIRES: u64 = 20;
pub const KEY_STREAM_IN: &str = "KEY_STREAM_IN:";
pub const KEY_SNAPSHOT_IMAGE: &str = "KEY_SNAPSHOT_IMAGE:";
