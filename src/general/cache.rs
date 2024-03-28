use std::collections::{BTreeMap, HashMap};
use common::tokio::time::Instant;

pub enum PlayType {
    Live,
    Back,
    Down,
}

impl PlayType {}

/// 目的：
/// 无人观看则关闭流，
/// 1.查看设备(device_id,channel_id)是否存在(流ID，流类型，观看人数)
struct State {
    steam_map: HashMap<(String, String, String), (PlayType, u32)>,
    //（ts,stream_id):(device_id,channel_id)
    expirations: BTreeMap<(Instant, String), (String, String)>,
}