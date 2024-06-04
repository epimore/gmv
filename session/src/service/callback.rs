use std::net::Ipv4Addr;
use log::error;
use reqwest::header;
use reqwest::header::HeaderMap;
use common::anyhow::anyhow;
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use crate::service::{ResMsg, StreamState};

const DROP_SSRC: &str = "/drop/ssrc";
const LISTEN_SSRC: &str = "/listen/ssrc";
const START_RECORD: &str = "/start/record";
const STOP_RECORD: &str = "/stop/record";
const START_PLAY: &str = "/start/play";
const STOP_PLAY: &str = "/stop/play";
const QUERY_STATE: &str = "/query/state";

fn build_uri_header(gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<(String, HeaderMap)> {
    let uri = format!("http://{}:{}", local_ip.to_string(), local_port);
    let mut headers = HeaderMap::new();
    headers.insert("gmv-token", header::HeaderValue::from_str(gmv_token).hand_err(|msg| error!("{msg}"))?);
    Ok((uri, headers))
}

pub async fn call_stream_state(opt_stream_id: Option<&String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<Vec<StreamState>> {
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    if let Some(stream_id) = opt_stream_id {
        uri = format!("{uri}{QUERY_STATE}?stream_id={stream_id}");
    } else {
        uri = format!("{uri}{QUERY_STATE}");
    }

    let body = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .hand_err(|msg| error!("{msg}"))?
        .get(&uri)
        .send()
        .await
        .hand_err(|msg| error!("{msg}"))?
        .json::<ResMsg<Vec<StreamState>>>()
        .await
        .hand_err(|msg| error!("{msg}"))?;
    return if body.code == 0 {
        if let Some(data) = body.data {
            return Ok(data);
        }
        Ok(Vec::new())
    } else {
        Err(SysErr(anyhow!("{}",body.msg)))
    };
}

pub async fn call_listen_ssrc(stream_id: &String, ssrc: &String, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
    let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
    uri = format!("{uri}{LISTEN_SSRC}?stream_id={stream_id}&ssrc={ssrc}");
    let body = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .hand_err(|msg| error!("{msg}"))?
        .get(&uri)
        .send()
        .await
        .hand_err(|msg| error!("{msg}"))?
        .json::<ResMsg<bool>>()
        .await
        .hand_err(|msg| error!("{msg}"))?;

    return if body.code == 0 {
        if let Some(data) = body.data {
            return Ok(data);
        }
        Ok(false)
    } else {
        Err(SysErr(anyhow!("{}",body.msg)))
    };
}