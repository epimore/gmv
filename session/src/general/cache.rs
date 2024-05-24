use std::collections::{BTreeMap, HashMap};
use common::tokio::time::Instant;

pub enum PlayType {
    Live,
    Back,
    Down,
}

//device_id/channel_id/ssrc/user_id/token

impl PlayType {}

/// 目的：todo 添加user_id、stream_id、device_id等关系，user拉取流时，需要通过session接口获取流地址：创建user与流的关系，stream回调查看是否具备权限，防止复制流地址获取流
/// 无人观看则关闭流，
/// 1.查看设备(device_id,channel_id)是否存在(流ID，流类型，观看人数)
/// 数据：Option<user_id>,device_id,channel_id,stream_id,stream_type,
struct State {
    steam_map: HashMap<(String, String, String), (PlayType, u32)>,
    //（ts,stream_id):(device_id,channel_id)
    expirations: BTreeMap<(Instant, String), (String, String)>,
}