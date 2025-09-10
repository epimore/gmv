use std::collections::HashMap;
use std::time::Duration;
use anyhow::__private::kind::AdhocKind;
use regex::Regex;
use rsip::prelude::{HeadersExt, UntypedHeader};
use rsip::{AbstractInput, Response, SipMessage};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, warn};
use base::tokio::sync::mpsc;
use base::tokio::time::Instant;
use shared::info::media_info_ext::{MediaExt, MediaType};
use crate::gb::handler::builder::{RequestBuilder, ResponseBuilder};
use crate::gb::core::event::{Container, EventSession, Ident};
use crate::gb::core::rw::RequestOutput;
use crate::state::model::{PtzControlModel, StreamMode};

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    pub async fn query_preset(device_id: &String, channel_id_opt: Option<&String>) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_preset(device_id, channel_id_opt).await?;
        RequestOutput::new(ident, msg, None).do_send()
    }
    pub async fn query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send()
    }
    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send()
    }
    pub async fn subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        RequestOutput::new(ident, msg, None).do_send()
    }
    pub async fn lazy_query_device_info(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_info(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }
    pub async fn lazy_query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::query_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }

    pub async fn lazy_subscribe_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::subscribe_device_catalog(device_id).await?;
        let when = Instant::now() + Duration::from_secs(2);
        EventSession::listen_event(&ident.clone(), when, Container::build_actor(ident, msg, None))
    }
}

pub struct CmdControl;

impl CmdControl {
    pub async fn control_ptz(ptz_control_model: &PtzControlModel) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::control_ptz(ptz_control_model).await?;
        RequestOutput::new(ident, msg, None).do_send()
    }

    pub async fn snapshot_image(device_id: &String, channel_id: &String, num: u8, interval: u8, uri: &String, session_id: &String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::control_snapshot_image(device_id, channel_id, num, interval, uri, session_id).await?;
        RequestOutput::new(ident, msg, None).do_send()
        // let device = GmvDevice::query_gmv_device_by_device_id(device_id).await?.ok_or_else(|| GlobalError::new_sys_error(&format!("未知设备: {device_id}"), |msg| error!("{msg}")))?;
        // match device.get_gb_version().as_deref() {
        //     Some("3.0") => {
        //         let (ident, msg) = RequestBuilder::control_snapshot_image(device_id, channel_id, num, interval, uri, session_id).await?;
        //         RequestOutput::new(ident, msg, None).do_send()
        //     }
        //     _ => {
        //         Err(GlobalError::new_sys_error(&format!("未知设备: {device_id}"), |msg| error!("{msg}")))
        //     }
        // }
    }
}

pub struct CmdNotify;

pub struct CmdStream;

impl CmdStream {
    pub async fn download_invite(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32, speed: u8)
                                 -> GlobalResult<(Response, MediaExt, String, String)> {
        let (ident, msg) = RequestBuilder::download(device_id, channel_id, dst_ip, dst_port, stream_mode, ssrc, st, et, speed)
            .await.hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(ident, msg).await
    }

    pub async fn play_back_invite(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32)
                                  -> GlobalResult<(Response, MediaExt, String, String)> {
        let (ident, msg) = RequestBuilder::playback(device_id, channel_id, dst_ip, dst_port, stream_mode, ssrc, st, et)
            .await.hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(ident, msg).await
    }
    pub async fn play_live_invite(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String)
                                  -> GlobalResult<(Response, MediaExt, String, String)> {
        let (ident, msg) = RequestBuilder::play_live_request(device_id, channel_id, dst_ip, dst_port, stream_mode, ssrc)
            .await.hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(ident, msg).await
    }

    pub fn invite_ack(device_id: &String, response: &Response) -> GlobalResult<(String, u32)> {
        let ack_request = RequestBuilder::build_ack_request_by_response(response)?;
        let call_id = ack_request.call_id_header().hand_log(|msg| warn!("{msg}"))?.value().to_string();
        let seq = ack_request.cseq_header().hand_log(|msg| warn!("{msg}"))?.seq().hand_log(|msg| warn!("{msg}"))?;
        RequestOutput::do_send_off(device_id, ack_request).hand_log(|msg| warn!("{msg}"))?;
        Ok((call_id, seq))
    }
    pub async fn play_speed(device_id: &String, channel_id: &String, speed: f32, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::speed(device_id, channel_id, speed, from_tag, to_tag, seq, call_id).await?;
        let (tx, mut rx) = mpsc::channel(10);
        RequestOutput::new(ident.clone(), msg, Some(tx)).do_send().hand_log(|msg| error!("未响应：{msg}"))?;
        if let Some((Some(res), _)) = rx.recv().await {
            if res.status_code.code() == 200 {
                EventSession::remove_event(&ident);
                return Ok(());
            }
            error!("speed: ident = {:?},channel_id = {},res = {}",&ident,channel_id,res.status_code);
        }
        EventSession::remove_event(&ident);
        return Err(GlobalError::new_biz_error(1000, "speed倍速未响应或超时", |msg| error!("{msg}")));
    }
    pub async fn play_seek(device_id: &String, channel_id: &String, seek: u32, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::seek(device_id, channel_id, seek, from_tag, to_tag, seq, call_id).await?;
        let (tx, mut rx) = mpsc::channel(10);
        RequestOutput::new(ident.clone(), msg, Some(tx)).do_send().hand_log(|msg| error!("未响应：{msg}"))?;
        if let Some((Some(res), _)) = rx.recv().await {
            if res.status_code.code() == 200 {
                EventSession::remove_event(&ident);
                return Ok(());
            }
            error!("seek: ident = {:?},channel_id = {},res = {}",&ident,channel_id,res.status_code);
        }
        EventSession::remove_event(&ident);
        return Err(GlobalError::new_biz_error(1000, "seek拖动未响应或超时", |msg| error!("{msg}")));
    }

    pub async fn play_bye(seq: u32, call_id: String, device_id: &String, channel_id: &String, from_tag: &str, to_tag: &str) -> GlobalResult<()> {
        let (ident, msg) = RequestBuilder::build_bye_request(seq, call_id, device_id, channel_id, from_tag, to_tag).await?;
        let (tx, mut rx) = mpsc::channel(10);

        RequestOutput::new(ident.clone(), msg, Some(tx)).do_send().hand_log(|msg| error!("未响应：{msg}"))?;

        if let Some((Some(res), _)) = rx.recv().await {
            if res.status_code.code() == 200 {
                EventSession::remove_event(&ident);
                return Ok(());
            }
            error!("关闭摄像机: ident = {:?},channel_id = {},res = {}",&ident,channel_id,res.status_code);
        }
        EventSession::remove_event(&ident);
        return Err(GlobalError::new_biz_error(1000, "关闭摄像机直播未响应或超时", |msg| error!("{msg}")));
    }
    //ps含音视频，todo cancel
    async fn invite_stream(ident: Ident, msg: SipMessage) -> GlobalResult<(Response, MediaExt, String, String)> {
        let (tx, mut rx) = mpsc::channel(10);
        RequestOutput::new(ident.clone(), msg, Some(tx)).do_send()?;
        while let Some((Some(res), _)) = rx.recv().await {
            let code = res.status_code.code();
            let code_msg = res.status_code.to_string();
            debug!("{ident:?} : {code} => {code_msg}");
            if code >= 300 {
                EventSession::remove_event(&ident);
                return Err(GlobalError::new_biz_error(3000, &code_msg, |msg| error!("{msg}")));
            }
            if code == 200 {
                if let Ok(ext) = Self::parse_sdp(res.body()) {
                    let from_tag = ResponseBuilder::get_tag_by_header_from(&res)?;
                    let to_tag = ResponseBuilder::get_tag_by_header_to(&res)?;
                    EventSession::remove_event(&ident);
                    return Ok((res, ext, from_tag, to_tag));
                }
                EventSession::remove_event(&ident);
                return Err(GlobalError::new_biz_error(1000, "摄像机响应rtpmap错误", |msg| error!("{msg}")));
            }
        }
        EventSession::remove_event(&ident);
        Err(GlobalError::new_biz_error(1000, "摄像机响应超时", |msg| error!("{msg}")))
    }

    fn parse_sdp(sdp: &Vec<u8>) -> GlobalResult<MediaExt> {
        let session = sdp_types::Session::parse(sdp).hand_log(|msg| error!("{msg}"))?;
        let re = Regex::new(r"\s+").hand_log(|msg| error!("{msg}"))?;
        let mut ext = MediaExt::default();
        for media in session.medias {
            if matches!(&*(media.media.trim().to_lowercase()),"video"|"audio") {
                if let Some(info) = media.get_first_attribute_value("rtpmap").hand_log(|msg| error!("{msg}"))?
                {
                    let trimmed = re.replace_all(info, " ").trim().to_string();
                    if let Some((play_code, payload)) = trimmed.split_once(' ') {
                        let type_code: u8 = play_code.trim().parse().hand_log(|msg| error!("{msg}"))?;
                        ext.type_code = type_code;
                        let vs: Vec<&str> = payload.trim().split('/').collect();
                        if vs.len() >= 2 {
                            ext.type_name = vs[0].trim().to_uppercase();
                            ext.clock_rate = vs[1].trim().parse().hand_log(|msg| error!("{msg}"))?;
                        }
                    }
                }
                if let Ok(Some(num)) = media.get_first_attribute_value("streamnumber") {
                    ext.stream_number = Some(num.trim().parse().hand_log(|msg| error!("{msg}"))?);
                }
            }
        }
        Self::extract_f_field(&mut ext, sdp);
        Ok(ext)
    }

    fn extract_f_field(me: &mut MediaExt, sdp: &Vec<u8>) {
        let sdp = str::from_utf8(sdp).unwrap();
        if let Some(f_field) = sdp.lines().find_map(|line| line.trim().strip_prefix("f=")) {
            let parts: Vec<&str> = f_field.split('/').map(|item| item.trim()).collect();
            if parts.len() == 9 && parts[0] == "v" && parts[5].ends_with("a") {
                if !parts[1].is_empty() { me.video_params.map_video_codec(parts[1]); }
                if !parts[2].is_empty() { me.video_params.map_resolution(parts[2]); }
                if !parts[3].is_empty() { me.video_params.map_fps(parts[3]); }
                if !parts[4].is_empty() { me.video_params.map_bitrate_type(parts[4]); }
                let p5 = parts[5].strip_suffix("a").unwrap().trim();
                if !p5.is_empty() { me.video_params.map_bitrate(p5); }
                if !parts[6].is_empty() { me.audio_params.map_audio_codec(parts[6]); }
                if !parts[7].is_empty() { me.audio_params.map_bitrate(parts[7]); }
                if !parts[8].is_empty() { me.audio_params.map_sample_rate(parts[8]); }
            }
        }
    }
}


#[cfg(test)]
#[allow(unused)]
mod test {
    use regex::Regex;
    use crate::gb::handler::cmd::CmdStream;

    #[test]
    fn test_parse_sdp() {
        let sdp_str = r#"v=0
o=34020000001110000009 0 0 IN IP4 192.168.110.254
s=Playback
c=IN IP4 192.168.110.254
t=1757289597 1757293200
m=video 62874 RTP/AVP 96
a=sendonly
a=rtpmap:96 PS/90000
y=0000004362
f=v/2/6/25/1/4096a///"#;
        let result = CmdStream::parse_sdp(&sdp_str.as_bytes().to_vec());


        println!("{:#?}", result);
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