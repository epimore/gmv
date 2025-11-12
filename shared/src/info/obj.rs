use crate::info::output::OutputEnum;
use base::constructor::New;
use base::serde::{Deserialize, Serialize};

//session
pub const PLAY_LIVING: &str = "/api/play/live/stream";
pub const PLAY_BACK: &str = "/api/play/back/stream";
pub const PLAY_SEEK: &str = "/api/play/back/seek";
pub const PLAY_SPEED: &str = "/api/play/back/speed";
pub const CONTROL_PTZ: &str = "/api/control/ptz";
pub const DOWNLOAD_MP4: &str = "/api/download/mp4";
pub const DOWNLOAD_STOP: &str = "/api/download/stop";
pub const DOWNING_INFO: &str = "/api/downing/info";
pub const RM_FILE: &str = "/api/rm/file";

pub const STREAM_REGISTER: &str = "/stream/register";
pub const INPUT_TIMEOUT: &str = "/stream/input/timeout";
pub const ON_PLAY: &str = "/on/play";
pub const OFF_PLAY: &str = "/off/play";
pub const STREAM_IDLE: &str = "/stream/idle";
pub const END_RECORD: &str = "/end/record";

//stream
pub const LISTEN_MEDIA: &str = "/listen/media";
pub const SDP_MEDIA: &str = "/sdp/media";
pub const STREAM_ONLINE: &str = "/stream/online";
pub const PLAY_PATH: &str = "/play/{stream_id}";
pub const RECORD_INFO: &str = "/record/info";
pub const CLOSE_OUTPUT: &str = "/close/output";

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct SingleParam<T> {
    pub param: T,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamInfoQo {
    pub ssrc: u32,
    pub output_enum: OutputEnum,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamState {
    pub base_stream_info: BaseStreamInfo,
    pub user_count: u32,
    // record_name: Option<String>,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct StreamRecordInfo {
    //录制完成时返回路径+文件名
    pub path_file_name: Option<String>,
    //单位kb,
    pub file_size: u64,
    //媒体流进度时间,方便计算进度，单位秒
    pub timestamp: u32,
    //录制状态，0-未开始，1-进行中，2-完成,3-失败
    pub state: u8,
    //每秒录制字节数
    // pub bytes_sec: usize,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamPlayInfo {
    pub base_stream_info: BaseStreamInfo,
    pub remote_addr: String,
    pub token: String,
    pub play_type: OutputEnum,
    //当前观看人数
    pub user_count: u32,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct BaseStreamInfo {
    pub rtp_info: RtpInfo,
    pub stream_id: String,
    pub in_time: u32,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct NetSource {
    pub remote_addr: String,
    pub protocol: String,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct RtpInfo {
    pub ssrc: u32,
    //媒体流源地址,tcp/udp
    pub origin_trans: Option<NetSource>,
    pub server_name: String,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamKey {
    pub ssrc: u32,
    pub stream_id: Option<String>,
}
