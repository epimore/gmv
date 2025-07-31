use common::serde::{Deserialize, Serialize};
use common::bytes::Bytes;
use common::cfg_lib::conf;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::{error, warn};
use common::constructor::{Get};
use common::serde_default;

//统一响应超时:单位毫秒
// pub const TIME_OUT: u64 = 8000;
// pub const HALF_TIME_OUT: u64 = 4000;
// //数据通道缓存大小
// pub const BUFFER_SIZE: usize = 64;
// //API接口根信息
// pub const INDEX: &str = r#"<!DOCTYPE html><html lang="en"><head>
//     <style>body{display:grid;place-items:center;height:100vh;margin:0;}<bof/style>
//     <metacharset="UTF - 8"><title>GMV</title></head>
// <body><div><h1>GMV:STREAM-SERVER</h1></div></body></html>"#;

//callback uri start
//ssrc流注册
pub const STREAM_IN: &str = "/stream/in";
//ssrc流无操作
pub const STREAM_IDLE: &str = "/stream/idle";
//播放流
pub const ON_PLAY: &str = "/on/play";
//关闭播放
pub const OFF_PLAY: &str = "/off/play";
//录制结束
pub const END_RECORD: &str = "/end/record";
//等待流超时
pub const STREAM_INPUT_TIMEOUT: &str = "/stream/input/timeout";
//callback uri end

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct ResMsg<T: Serialize> {
    code: u16,
    msg: String,
    data: Option<T>,
}

impl<T: Serialize> ResMsg<T> {
    pub fn build_success() -> Self {
        Self { code: 200, msg: "success".to_string(), data: None }
    }
    pub fn build_failed() -> Self {
        Self { code: 500, msg: "failed".to_string(), data: None }
    }

    pub fn build_failed_by_msg(msg: String) -> Self {
        Self { code: 500, msg, data: None }
    }

    pub fn define_res(code: u16, msg: String) -> Self {
        Self { code, msg, data: None }
    }

    pub fn to_json(&self) -> GlobalResult<String> {
        let json_str = common::serde_json::to_string(self).hand_log(|msg| error!("{msg}"))?;
        Ok(json_str)
    }

    pub fn build_success_data(data: T) -> Self {
        Self { code: 200, msg: "success".to_string(), data: Some(data) }
    }
}

#[derive(Debug, Get, Clone, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "server")]
pub struct ServerConf {
    name: String,
    rtp_port: u16,
    rtcp_port: u16,
    http_port: u16,
    hook_uri: String,
}
serde_default!(default_name, String, "stream-node-1".to_string());
serde_default!(default_rtp_port, u16, 18568);
serde_default!(default_rtcp_port, u16, 18569);
serde_default!(default_http_port, u16, 18570);
serde_default!(default_hook_uri, String, "http://127.0.0.1:18567".to_string());
impl ServerConf {
    pub fn init_by_conf() -> Self {
        ServerConf::conf()
    }
}

pub const AV_IO_CTX_BUFFER_SIZE: u16 = 1024 * 4;


#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
pub enum Coder { //av1??
    //video
    // PS,
    // MPEG4,
    //sps,pps,idr
    H264(Option<Bytes>, Option<Bytes>, bool),
    // SVAC_V,
    H265,
    //AUDIO
    G711,
    // SVAC_A,
    // G723_1,
    // G729,
    // G722_1,
    // AAC,
}

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Media { //av1??
    //video
    PS,
    // MPEG4,
    //sps,pps,idr
    H264,
    // SVAC_V,
    // H265,
    // //AUDIO
    // G711,
    // SVAC_A,
    // G723_1,
    // G729,
    // G722_1,
    // AAC,
}

impl Media {
    pub fn build(ident_str: &str) -> GlobalResult<Self> {
        match ident_str {
            "PS" => { Ok(Self::PS) }
            "H264" => { Ok(Self::H264) }
            other => {
                return Err(GlobalError::new_sys_error(&format!("暂不支持的数据类型-{other}"), |msg| warn!("{msg}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {}