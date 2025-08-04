use common::constructor::New;
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::serde::{Deserialize, Serialize};

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamState {
    pub base_stream_info: BaseStreamInfo,
    pub user_count: u32,
    // record_name: Option<String>,
}

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamRecordInfo {
    pub file_name: Option<String>,
    //单位kb,录制完成时统计文件大小
    pub file_size: Option<u64>,
    //媒体流进度时间,方便计算进度，单位秒
    pub timestamp: u32,
    //每秒录制字节数
    pub bytes_sec: usize,
}

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamPlayInfo {
    pub base_stream_info: BaseStreamInfo,
    pub remote_addr: String,
    pub token: String,
    pub play_type: HttpStreamType,
    //当前观看人数
    pub user_count: u32,
}

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct BaseStreamInfo {
    pub rtp_info: RtpInfo,
    pub stream_id: String,
    pub in_time: u32,
}

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct NetSource {
    pub remote_addr: String,
    pub protocol: String,
}

#[derive(New, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpInfo {
    pub ssrc: u32,
    //媒体流源地址,tcp/udp
    pub origin_trans: Option<NetSource>,
    pub server_name: String,
}

//Mp4下载及rtp推流；由发起方控制，不回调鉴权
//    Rtmp,
//     Rtsp,
//     WebRtc,
#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum HttpStreamType {
    HttpFlv,
    Hls,
    Dash,
}
impl HttpStreamType {
    pub fn from_str(s: &str) -> GlobalResult<HttpStreamType> {
        match s {
            "flv" => Ok(HttpStreamType::HttpFlv),
            "ts" => Ok(HttpStreamType::Hls),
            // "dash" => Some(StreamType::Dash),
            // "rtmp" => Some(StreamType::Rtmp),
            // "rtsp" => Some(StreamType::Rtsp),
            // "webrtc" => Some(StreamType::WebRtc),
            other => Err(GlobalError::new_sys_error(&format!("unknown stream type {}", other), |msg| error!("{msg}"))),
        }
    }

    pub fn to_string(&self) -> String {
        match self {
            HttpStreamType::HttpFlv => "http_flv".to_string(),
            HttpStreamType::Hls => "hls".to_string(),
            HttpStreamType::Dash => "dash".to_string(),
        }
    }
}