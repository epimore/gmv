use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use constructor::{Get, New};

pub mod handler;
mod callback;

pub const EXPIRES: u64 = 8;
pub const RELOAD_EXPIRES: u64 = 2;

#[derive(Serialize, Object, Deserialize, Debug)]
pub struct ResMsg<T: Serialize + Sync + Send + poem_openapi::types::Type + poem_openapi::types::ToJSON + poem_openapi::types::ParseFromJSON> {
    code: i8,
    msg: String,
    data: Option<T>,
}

#[derive(New, Serialize, Object, Deserialize, Get, Debug)]
pub struct StreamState {
    base_stream_info: BaseStreamInfo,
    flv_play_count: u32,
    hls_play_count: u32,
    record_name: Option<String>,
}

#[derive(New, Serialize, Object, Deserialize, Get, Debug)]
pub struct BaseStreamInfo {
    rtp_info: RtpInfo,
    stream_id: String,
    in_time: u32,
}

#[derive(New, Object, Serialize, Deserialize, Get, Debug)]
pub struct RtpInfo {
    ssrc: u32,
    //tcp/udp
    protocol: Option<String>,
    //媒体流源地址
    origin_addr: Option<String>,
    server_name: String,
}