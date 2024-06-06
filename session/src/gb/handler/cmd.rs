use std::collections::HashMap;
use std::time::Duration;
use log::{error, info};
use regex::Regex;
use rsip::prelude::HeadersExt;
use rsip::Response;
use common::clap::builder::IntoResettable;
use common::err::{GlobalError, GlobalResult, TransError};
use common::tokio::sync::mpsc;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::time::Instant;
use crate::gb::handler::builder::{RequestBuilder, ResponseBuilder};
use crate::gb::RWSession;
use crate::gb::shared::event::{Container, EventSession};
use crate::gb::shared::rw::RequestOutput;
use crate::general::model::StreamMode;

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    pub async fn query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send().await
    }
    pub async fn lazy_query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
    pub async fn lazy_query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
    pub async fn lazy_subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None)).await
    }
}

pub struct CmdControl;

pub struct CmdNotify;

pub struct CmdStream;

impl CmdStream {
    pub async fn play_live_invite(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String)
                                  -> GlobalResult<(Response, HashMap<String, String>)> {
        let (ident, msg) = RequestBuilder::play_live_request(device_id, channel_id, dst_ip, dst_port, stream_mode, ssrc)
            .await.hand_log_err()?;
        let (tx, mut rx) = mpsc::channel(100);
        RequestOutput::new(ident.clone(), msg, Some(tx)).do_send().await?;
        while let Some((Some(res), _)) = rx.recv().await {
            let code = res.status_code.code();
            let code_msg = res.status_code.to_string();
            info!("{ident:?} : {code} => {code_msg}");
            if code >= 300 {
                EventSession::remove_event(&ident).await;
                return Err(GlobalError::new_biz_error(3000, &code_msg, |msg| error!("{msg}")));
            }
            if code == 200 {
                let to_tag = ResponseBuilder::get_tag_by_header_to(&res)?;
                let from_tag = ResponseBuilder::get_tag_by_header_from(&res)?;
                let session = sdp_types::Session::parse(res.body()).unwrap();
                info!("{ident:?} :{:#?}",&session);
                let re = Regex::new(r"\s+").unwrap();
                let mut media_map = HashMap::new();
                for attr in session.attributes {
                    if attr.attribute.eq("rtpmap") {
                        if let Some(info) = attr.value {
                            if let Some((key, val)) = re.replace_all(info.trim(), " ").split_once(" ") {
                                media_map.insert(key.to_string(), val.to_string());
                            }
                        }
                    }
                }
                return Ok((res, media_map));
            }
        }
        EventSession::remove_event(&ident).await;
        return Err(GlobalError::new_biz_error(1000, "摄像机响应超时", |msg| error!("{msg}")));
    }

    pub async fn play_live_ack(device_id: &String, response: &Response) -> GlobalResult<(String, u32)> {
        let ack_request = RequestBuilder::build_ack_request_by_response(response)?;
        let call_id = ack_request.call_id_header().hand_log_err()?.to_string();
        let seq = ack_request.cseq_header().hand_log_err()?.seq().hand_log_err()?;
        RequestOutput::do_send_off(device_id, ack_request).await.hand_log_err()?;
        Ok((call_id, seq))
    }
}


#[cfg(test)]
mod test {
    use regex::Regex;

    #[test]
    fn test_parse_sdp() {
        let sdp_str = "v=0
o=33010602001310019325 0 0 IN IP4 10.64.49.44
s=Play
c=IN IP4 10.64.49.218
t=0 0
m=video 5514 RTP/AVP 96
a=rtpmap:96 PS/90000
a=sendonly
y=0060205514";
        let session = sdp_types::Session::parse(sdp_str.as_ref()).unwrap();
        println!("{:#?}", session);
    }


    #[test]
    fn test_str_blank() {
        let str0 = " 96   PS/90000 ";
        let str1 = "96 PS/90000";
        let str2 = "96  PS/90000";
        let str3 = " 96 PS/90000";
        let str4 = "96 PS/90000 ";
        let re = Regex::new(r"\s+").unwrap();
        let s0 = str0.trim().replace("  ", " ");

        println!("{s0}");
    }
}