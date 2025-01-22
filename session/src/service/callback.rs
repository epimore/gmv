use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use reqwest::header;
use reqwest::header::HeaderMap;
use common::serde::{Deserialize, Serialize};
use common::anyhow::anyhow;
use common::exception::{GlobalResult, TransError};
use common::exception::GlobalError::SysErr;
use common::log::error;

use crate::service::{EXPIRES, ResMsg};

#[allow(dead_code)]
const DROP_SSRC: &str = "/drop/ssrc";
#[allow(dead_code)]
const LISTEN_SSRC: &str = "/listen/ssrc";
#[allow(dead_code)]
const START_RECORD: &str = "/start/record";
#[allow(dead_code)]
const STOP_RECORD: &str = "/stop/record";
#[allow(dead_code)]
const START_PLAY: &str = "/start/play";
#[allow(dead_code)]
const STOP_PLAY: &str = "/stop/play";
#[allow(dead_code)]
const QUERY_STATE: &str = "/query/state";
#[allow(dead_code)]
const RTP_MEDIA: &str = "/rtp/media";

fn build_uri_header(gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<(String, HeaderMap)> {
    let uri = format!("http://{}:{}", local_ip.to_string(), local_port);
    let mut headers = HeaderMap::new();
    headers.insert("gmv-token", header::HeaderValue::from_str(gmv_token).hand_log(|msg| error!("{msg}"))?);
    Ok((uri, headers))
}

pub async fn get_stream_count(opt_stream_id: Option<&String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<u32> {
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    if let Some(stream_id) = opt_stream_id {
        uri = format!("{uri}{QUERY_STATE}?stream_id={stream_id}");
    } else {
        uri = format!("{uri}{QUERY_STATE}");
    }

    let res = reqwest::Client::builder()
        .timeout(Duration::from_secs(EXPIRES))
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .get(&uri)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    return if res.status().is_success() {
        let body = res.json::<ResMsg<u32>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        if body.code == 200 {
            if let Some(data) = body.data {
                return Ok(data);
            }
        }
        Err(SysErr(anyhow!("{}",body.msg)))
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(crate = "common::serde")]
pub struct SsrcLisDto {
    pub ssrc: u32,
    pub stream_id: String,
    //当为None时，默认配置,负数-立马关闭
    pub expires: Option<i32>,
    pub flv: Option<bool>,
    pub hls: Option<bool>,
}

pub async fn call_listen_ssrc(stream_id: String, ssrc: &String, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
    let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let ssrc_lis_dto = SsrcLisDto {
        ssrc,
        stream_id,
        ..Default::default()
    };
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    uri = format!("{uri}{LISTEN_SSRC}");
    let res = reqwest::Client::builder()
        .timeout(Duration::from_secs(EXPIRES))
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .post(&uri)
        .json(&ssrc_lis_dto)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    return if res.status().is_success() {
        let body = res.json::<ResMsg<bool>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        Ok(body.code == 200)
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
struct RtpMap {
    ssrc: u32,
    map: HashMap<u8, String>,
}

pub async fn ident_rtp_media_info(ssrc: &String, map: HashMap<u8, String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
    let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let rtp_map = RtpMap { ssrc, map };
    let (uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    let res = reqwest::Client::builder()
        .timeout(Duration::from_secs(EXPIRES))
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .post(format!("{uri}{RTP_MEDIA}"))
        .json(&rtp_map)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    return if res.status().is_success() {
        let body = res.json::<ResMsg<bool>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        Ok(body.code == 200)
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}