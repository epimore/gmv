use serde::{Deserialize, Serialize};
use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::log::{error};
use common::yaml_rust::Yaml;
use constructor::{Get};

//统一响应超时:单位毫秒
pub const TIME_OUT: u64 = 8000;
pub const HALF_TIME_OUT: u64 = 4000;
//数据通道缓存大小
pub const BUFFER_SIZE: usize = 8;
//API接口根信息
pub const INDEX: &str = r#"<!DOCTYPE html><html lang="en"><head>
    <style>body{display:grid;place-items:center;height:100vh;margin:0;}<bof/style>
    <metacharset="UTF - 8"><title>GMV</title></head>
<body><div><h1>GMV:STREAM-SERVER</h1></div></body></html>"#;

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
pub struct ResMsg<T: Serialize> {
    code: i8,
    msg: String,
    data: Option<T>,
}

impl<T: Serialize> ResMsg<T> {
    pub fn build_success() -> Self {
        Self { code: 0, msg: "success".to_string(), data: None }
    }
    pub fn build_failed() -> Self {
        Self { code: -1, msg: "failed".to_string(), data: None }
    }

    pub fn build_failed_by_msg(msg: String) -> Self {
        Self { code: -1, msg, data: None }
    }

    pub fn define_res(code: i8, msg: String) -> Self {
        Self { code, msg, data: None }
    }

    pub fn to_json(&self) -> GlobalResult<String> {
        let json_str = serde_json::to_string(self).hand_log(|msg| error!("{msg}"))?;
        Ok(json_str)
    }

    pub fn build_success_data(data: T) -> Self {
        Self { code: 0, msg: "success".to_string(), data: Some(data) }
    }
}

#[derive(Debug, Get, Clone)]
pub struct ServerConf {
    name: String,
    rtp_port: u16,
    rtcp_port: u16,
    http_port: u16,
    hook_uri: String,
}

impl ServerConf {
    pub fn build(cfg: &Yaml) -> Self {
        if cfg.is_badvalue() || cfg["server"].is_badvalue() {
            Self {
                name: "stream-node-1".to_string(),
                rtp_port: 18568,
                rtcp_port: 18569,
                http_port: 18570,
                hook_uri: "http://127.0.0.1:18567".to_string(),
            }
        } else {
            let server = &cfg["server"];
            Self {
                name: server["name"].as_str().map(|str| str.to_string()).unwrap_or("stream-node-1".to_string()),
                rtp_port: server["rtp-port"].as_i64().unwrap_or(18568) as u16,
                rtcp_port: server["rtcp-port"].as_i64().unwrap_or(18569) as u16,
                http_port: server["http-port"].as_i64().unwrap_or(18570) as u16,
                hook_uri: server["hook-uri"].as_str().map(|str| str.to_string()).unwrap_or("http://127.0.0.1:18567".to_string()),
            }
        }
    }
}

pub const AV_IO_CTX_BUFFER_SIZE: u16 = 1024 * 4;


#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
pub enum Coder {
    //video
    PS,
    MPEG4,
    //sps,pps,idr
    H264(Option<Bytes>, Option<Bytes>, bool),
    SVAC_V,
    H265,
    //AUDIO
    G711,
    SVAC_A,
    G723_1,
    G729,
    G722_1,
    AAC,
}
//
// impl Coder {
//     pub fn gb_check(tp: u8) -> GlobalResult<Self> {
//         match tp {
//             //video
//             //ps
//             96 => { Ok(Self::PS) }
//             //mpeg-4
//             97 => { Ok(Self::MPEG4) }
//             //h264
//             98 => { Ok(Self::H264) }
//             //svac
//             99 => { Ok(Self::SVAC_V) }
//             //h265
//             100 => { Ok(Self::H265) }
//             //audio
//             //g711
//             8 => { Ok(Self::G711) }
//             //svac
//             20 => { Ok(Self::SVAC_A) }
//             //g723-1
//             4 => { Ok(Self::G723_1) }
//             //g729
//             18 => { Ok(Self::G729) }
//             //g722.1
//             9 => { Ok(Self::G722_1) }
//             //aac
//             102 => { Ok(Self::AAC) }
//             _ => {
//                 Err(GlobalError::new_biz_error(4004, &*format!("rtp type = {tp},GB28181未定义类型。"), |msg| debug!("{msg}")))
//             }
//         }
//     }
//
//     pub fn impl_check(tp: u8) -> GlobalResult<Self> {
//         match tp {
//             //video
//             //ps
//             96 => { Ok(Self::PS) }
//             //h264
//             98 => { Ok(Self::H264) }
//             _ => {
//                 Self::gb_check(tp)
//                     .and_then(|v|
//                     Err(GlobalError::new_biz_error(4005, &*format!("rtp type = {:?},系统暂不支持。", v), |msg| debug!("{msg}"))))
//             }
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use crate::general::mode::ServerConf;

    #[test]
    pub fn test_build_stream() {
        let binding = common::get_config();
        let cfg = binding.get(0).unwrap();
        println!("{:?}", ServerConf::build(cfg));
    }
}