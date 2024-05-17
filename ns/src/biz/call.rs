use std::net::SocketAddr;
use std::time::Duration;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use common::anyhow::Error;
use common::err::{GlobalResult, TransError};
use common::log::error;
use constructor::{Get, New};
use log::{log, warn};
use crate::general::mode;
use crate::general::mode::TIME_OUT;
use crate::state::cache;

#[derive(Deserialize)]
struct RespBo {
    code: i16,
    msg: Option<String>,
}

#[derive(New, Serialize, Get)]
pub struct RtpInfo {
    ssrc: u32,
    //tcp/udp
    protocol: String,
    //媒体流源地址
    origin_addr: String,
    server_name: String,
}

impl RtpInfo {
    //未知流，每隔8秒提示一次
    pub async fn stream_unknown(&self) {}
}

#[derive(New, Serialize, Get)]
pub struct BaseStreamInfo {
    rtp_info: RtpInfo,
    stream_id: String,
    in_time: u32,
}

impl BaseStreamInfo {
    //当接收到输入流时进行回调
    pub async fn stream_in(&self) -> Option<bool> {
        let client = reqwest::Client::new();
        let uri = format!("{} {}", cache::get_server_conf().get_hook_uri(), mode::STREAM_IN);
        let res = client.post(uri)
            .timeout(Duration::from_millis(TIME_OUT))
            .json(self).send().await
            .hand_err(|msg| error!("{msg}"))
            .ok().map(|res| res.status().is_success());
        res
    }
    // //当流闲置时（无观看、无录制），依旧接收到ssrc流输入时，间隔8秒回调一次
    // pub async fn stream_idle(&self) {}
}

#[derive(New, Serialize, Get)]
pub struct StreamPlayInfo {
    base_stream_info: BaseStreamInfo,
    remote_addr: SocketAddr,
    token: String,
    //0-flv,1-hls
    play_type: String,
    //当前观看人数
    flv_play_count: u32,
    hls_play_count: u32,
}

impl StreamPlayInfo {
    //当用户访问播放流时进行回调（可用于鉴权）
    pub async fn on_play(&self) -> Option<bool> {
        let client = reqwest::Client::new();
        let uri = format!("{} {}", cache::get_server_conf().get_hook_uri(), mode::ON_PLAY);
        let res = client.post(uri)
            .timeout(Duration::from_millis(TIME_OUT))
            .json(self).send().await
            .hand_err(|msg| error!("{msg}"));
        match res {
            Ok(resp) => {
                match (resp.status().is_success(), resp.json::<RespBo>().await) {
                    (true, Ok(RespBo { code: 0, msg: _ })) => {
                        Some(true)
                    }
                    _ => {
                        Some(false)
                    }
                }
            }
            Err(_) => {
                None
            }
        }
    }

    //当用户断开播放时进行回调
    pub async fn off_play(&self) -> Option<bool> {
        let client = reqwest::Client::new();
        let uri = format!("{} {}", cache::get_server_conf().get_hook_uri(), mode::OFF_PLAY);
        let res = client.post(uri)
            .timeout(Duration::from_millis(TIME_OUT))
            .json(self).send().await
            .hand_err(|msg| error!("{msg}"));
        match res {
            Ok(resp) => {
                match (resp.status().is_success(), resp.json::<RespBo>().await) {
                    (true, Ok(RespBo { code: 1, msg: _ })) => {
                        Some(true)
                    }
                    _ => {
                        Some(false)
                    }
                }
            }
            Err(_) => {
                None
            }
        }
    }
}

#[derive(New, Serialize)]
pub struct StreamRecordInfo {
    base_stream_info: BaseStreamInfo,
    file_path: String,
    file_name: String,
    //单位kb
    file_size: u32,
}

impl StreamRecordInfo {
    //当流录制完成时进行回调
    pub async fn end_record(&self) {}
}

#[derive(New, Serialize)]
pub struct StreamState {
    base_stream_info: BaseStreamInfo,
    flv_play_count: u32,
    hls_play_count: u32,
    record_name: Option<String>,
}

impl StreamState {
    //当等待输入流超时时进行回调
    pub async fn stream_input_timeout(&self) -> Option<bool> {
        let client = reqwest::Client::new();
        let uri = format!("{} {}", cache::get_server_conf().get_hook_uri(), mode::STREAM_INPUT_TIMEOUT);
        let res = client.post(uri)
            .timeout(Duration::from_millis(TIME_OUT))
            .json(self).send().await
            .hand_err(|msg| error!("{msg}"));
        match res {
            Ok(resp) => {
                match (resp.status().is_success(), resp.json::<RespBo>().await) {
                    (true, Ok(_)) => {
                        Some(true)
                    }
                    _ => {
                        Some(false)
                    }
                }
            }
            Err(_) => {
                None
            }
        }
    }
}