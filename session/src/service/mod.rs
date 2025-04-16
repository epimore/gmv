use poem_openapi::{Enum, Object};
use common::serde::{Deserialize, Serialize};

use common::constructor::{Get, New};

pub mod handler;
pub mod callback;
pub mod biz;

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

#[derive(Object, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamRecordInfo {
    pub file_name: Option<String>,
    //单位kb,录制完成时统计文件大小
    pub file_size: Option<u64>,
    //媒体流原始时间,方便计算进度
    pub timestamp: u32,
    //每秒录制字节数
    pub bytes_sec: usize,
}
