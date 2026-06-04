use crate::info::output::OutputEnum;
use base::constructor::New;
use base::serde::{Deserialize, Serialize};

//session
pub const PLAY_LIVING: &str = "/play/live/stream";
pub const PLAY_BACK: &str = "/play/back/stream";
pub const PLAY_SEEK: &str = "/play/back/seek";
pub const PLAY_SPEED: &str = "/play/back/speed";
pub const CONTROL_PTZ: &str = "/control/ptz";
pub const DOWNLOAD_MP4: &str = "/download/mp4";
pub const DOWNLOAD_STOP: &str = "/download/stop";
pub const DOWNING_INFO: &str = "/downing/info";
pub const RM_FILE: &str = "/rm/file";
pub const TALK_START: &str = "/talk/start";
pub const TALK_STOP: &str = "/talk/stop";

pub const STREAM_REGISTER: &str = "/stream/register";
pub const INPUT_TIMEOUT: &str = "/stream/input/timeout";
pub const ON_PLAY: &str = "/on/play";
pub const OFF_PLAY: &str = "/off/play";
pub const STREAM_IDLE: &str = "/stream/idle";
pub const END_RECORD: &str = "/end/record";
pub const TALK_CLOSED: &str = "/talk/closed";

//stream
pub const LISTEN_MEDIA: &str = "/listen/media";
pub const SDP_MEDIA: &str = "/sdp/media";
pub const STREAM_ONLINE: &str = "/stream/online";
pub const PLAY_PATH: &str = "/play/{stream_id}";
pub const RECORD_INFO: &str = "/record/info";
pub const CLOSE_OUTPUT: &str = "/close/output";
pub const TALK_OPEN: &str = "/talk/open";
pub const TALK_ANSWER: &str = "/talk/answer";
pub const TALK_CLOSE: &str = "/talk/close";
pub const TALK_INPUT_PREFIX: &str = "/talk/input";
pub const TALK_INPUT_PATH: &str = "/talk/input/{talk_id}";

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
    ///录制完成时返回路径+文件名
    pub path_file_name: Option<String>,
    //单位kb,
    pub file_size: u64,
    ///媒体流进度时间,方便计算进度，单位秒
    pub timestamp: u32,
    ///录制状态，0-未开始，1-进行中，2-完成,3-失败
    pub state: u8,
    //每秒录制字节数
    // pub bytes_sec: usize,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamPlayInfo {
    pub base_stream_info: BaseStreamInfo,
    pub remote_addr: Option<String>,
    pub token: String,
    pub play_type: OutputEnum,
    // //当前观看人数
    // pub user_count: u32,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub enum InTimeoutEventRes {
    KeepAlive, //保活ssrc所有资源，进入下次超时扫码;
    CloseAll,  //关闭释放ssrc所有资源;
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub enum OutputEventRes {
    KeepMuxer,  //保留ssrc当前输出格式资源;
    CloseMuxer, //关闭释放ssrc当前输出格式资源;
    CloseAll,   //关闭释放ssrc所有资源;
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct OutputStreamInfo {
    pub base_stream_info: BaseStreamInfo,
    pub play_type: OutputEnum,
    //当前观看人数
    pub user_count: u32,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct BaseStreamInfo {
    pub rtp_info: RtpInfo,
    pub stream_id: String,
    pub in_time: u64,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RegisterStreamInfo {
    pub base_stream_info: BaseStreamInfo,
    pub code: u16,
    pub msg: Option<String>,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct NetSource {
    pub remote_addr: String,
    pub protocol: String,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RtpInfo {
    pub ssrc: u32,
    //媒体流源地址,tcp/udp
    pub origin_trans: Option<NetSource>,
    pub server_name: String,
    pub proxy_addr: String,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamKey {
    pub ssrc: u32,
    pub stream_id: Option<String>,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct TalkStartModel {
    pub device_id: String,
    pub channel_id: Option<String>,
    pub transport: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channel_count: Option<u8>,
    pub frame_duration_ms: Option<u16>,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkInfo {
    pub talk_id: String,
    pub input_url: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub frame_duration_ms: u16,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct TalkStopModel {
    pub talk_id: String,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkClosedEvent {
    pub talk_id: String,
    pub reason: String,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkOpenReq {
    pub talk_id: String,
    pub ssrc: u32,
    pub token: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub payload_type: u8,
    pub frame_duration_ms: u16,
    pub input_timeout_secs: u16,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkOpenResp {
    pub talk_id: String,
    pub input_url: String,
    pub rtp_port: u16,
    pub codec: String,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub payload_type: u8,
    pub frame_duration_ms: u16,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkAnswerReq {
    pub talk_id: String,
    pub device_ip: String,
    pub device_port: u16,
    pub protocol: String,
    pub payload_type: u8,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct TalkCloseReq {
    pub talk_id: String,
}
