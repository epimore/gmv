use std::collections::HashMap;
use std::net::Ipv4Addr;

use reqwest::header;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use common::anyhow::anyhow;
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::error;

use crate::service::{ResMsg, StreamState};

const DROP_SSRC: &str = "/drop/ssrc";
const LISTEN_SSRC: &str = "/listen/ssrc";
const START_RECORD: &str = "/start/record";
const STOP_RECORD: &str = "/stop/record";
const START_PLAY: &str = "/start/play";
const STOP_PLAY: &str = "/stop/play";
const QUERY_STATE: &str = "/query/state";

const RTP_MEDIA: &str = "/rtp/media";

fn build_uri_header(gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<(String, HeaderMap)> {
    let uri = format!("http://{}:{}", local_ip.to_string(), local_port);
    let mut headers = HeaderMap::new();
    headers.insert("gmv-token", header::HeaderValue::from_str(gmv_token).hand_log(|msg| error!("{msg}"))?);
    Ok((uri, headers))
}

pub async fn call_stream_state(opt_stream_id: Option<&String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<Vec<StreamState>> {
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    if let Some(stream_id) = opt_stream_id {
        uri = format!("{uri}{QUERY_STATE}?stream_id={stream_id}");
    } else {
        uri = format!("{uri}{QUERY_STATE}");
    }

    let res = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .get(&uri)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    return if res.status().is_success() {
        let body = res.json::<ResMsg<Vec<StreamState>>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        if body.code == 200 {
            if let Some(data) = body.data {
                return Ok(data);
            }
        }
        Ok(Vec::new())
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}

pub async fn call_listen_ssrc(stream_id: &String, ssrc: &String, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    uri = format!("{uri}{LISTEN_SSRC}?stream_id={stream_id}&ssrc={ssrc}");
    println!("listen ssrc uri = {}", &uri);
    let res = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .get(&uri)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    println!("res = {:?}", &res);
    return if res.status().is_success() {
        let body = res.json::<ResMsg<bool>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        println!("body = {:?}", &body);
        Ok(body.code == 200)
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}

#[derive(Serialize, Deserialize, Debug)]
struct RtpMap {
    ssrc: u32,
    map: HashMap<u8, String>,
}

pub async fn ident_rtp_media_info(ssrc: &String, map: HashMap<u8, String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
    let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let rtp_map = RtpMap { ssrc, map };
    let (uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    let res = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .hand_log(|msg| error!("{msg}"))?
        .post(format!("{uri}{RTP_MEDIA}"))
        .json(&rtp_map)
        .send()
        .await
        .hand_log(|msg| error!("{msg}"))?;
    println!("res = {:?}", &res);
    return if res.status().is_success() {
        let body = res.json::<ResMsg<bool>>()
            .await
            .hand_log(|msg| error!("{msg}"))?;
        println!("body = {:?}", &body);
        Ok(body.code == 200)
    } else {
        Err(SysErr(anyhow!("{}",res.status().to_string())))
    };
}