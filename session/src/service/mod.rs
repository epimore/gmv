use poem_openapi::{Enum, Object};
use common::serde::{Deserialize, Serialize};

use common::constructor::{Get, New};

pub mod handler;
mod callback;
pub mod control;

pub const EXPIRES: u64 = 8;
pub const RELOAD_EXPIRES: u64 = 2;

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Enum)]
#[serde(crate = "common::serde")]
pub enum PlayType {
    Flv,
    Hls,
}

#[derive(Serialize, Deserialize, Debug, Object)]
#[serde(crate = "common::serde")]
pub struct ResMsg<T: Serialize + Sync + Send + poem_openapi::types::Type + poem_openapi::types::ToJSON + poem_openapi::types::ParseFromJSON> {
    code: u16,
    msg: String,
    data: Option<T>,
}

#[derive(New, Serialize, Object, Deserialize, Get, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamState {
    base_stream_info: BaseStreamInfo,
    user_count: u32,
    // record_name: Option<String>,
}

#[derive(New, Serialize, Object, Deserialize, Get, Debug)]
#[serde(crate = "common::serde")]
pub struct BaseStreamInfo {
    rtp_info: RtpInfo,
    stream_id: String,
    in_time: u32,
}

#[derive(New, Serialize, Get, Deserialize, Object, Debug)]
#[serde(crate = "common::serde")]
pub struct NetSource {
    remote_addr: String,
    protocol: String,
}

#[derive(New, Object, Serialize, Deserialize, Get, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpInfo {
    ssrc: u32,
    //媒体流源地址,tcp/udp
    origin_trans: Option<NetSource>,
    // //tcp/udp
    // protocol: Option<String>,
    // //媒体流源地址
    // origin_addr: Option<String>,
    server_name: String,
}

#[derive(New, Object, Serialize, Deserialize, Get, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamPlayInfo {
    base_stream_info: BaseStreamInfo,
    remote_addr: String,
    token: String,
    play_type: PlayType,
    //当前观看人数
    user_count: u32,
}

#[derive(New, Object, Serialize, Deserialize, Get, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamRecordInfo {
    base_stream_info: BaseStreamInfo,
    file_path: String,
    file_name: String,
    //单位kb
    file_size: u32,
}