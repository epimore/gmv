pub mod api_serv;
mod broadcast;
pub mod broadcast_close;
pub mod cloud_recording;
pub mod dialog_epoch;
pub mod dialog_recovery;
pub mod edge_serv;
pub mod hook_serv;
pub mod playback_presence;
pub mod record_query;
pub mod stream_close;
pub mod stream_rpc;

pub const EXPIRES: u64 = 8;
pub const SNAPSHOT_IDLE_EXPIRES: u64 = 20;
pub const KEY_STREAM_IN: &str = "KEY_STREAM_IN:";
pub const KEY_SNAPSHOT_IMAGE: &str = "KEY_SNAPSHOT_IMAGE:";
