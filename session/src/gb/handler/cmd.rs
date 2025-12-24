use crate::gb::core::rw::SipRequestOutput;
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::depot::default_response_callback;
use crate::gb::handler::builder::RequestBuilder;
use crate::state::model::{PtzControlModel, TransMode};
use anyhow::anyhow;
use base::exception::GlobalError::SysErr;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::net::state::Association;
use base::tokio::sync::oneshot;
use regex::Regex;
use rsip::prelude::UntypedHeader;
use rsip::{Request, Response};
use shared::info::media_info_ext::MediaExt;

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    pub async fn query_device_info(device_id: &String) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::build_query_device_info(device_id).await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("query_device_info")
            .await;
        Ok(())
    }

    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::query_device_catalog(device_id).await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("query_device_catalog")
            .await;
        Ok(())
    }

    pub async fn subscribe_device_catalog(device_id: &String, expire: u32) -> GlobalResult<()> {
        let (request, association) =
            RequestBuilder::subscribe_device_catalog(device_id, expire).await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("subscribe_device_catalog")
            .await;
        Ok(())
    }
}

pub struct CmdControl;

impl CmdControl {
    pub async fn control_ptz(ptz_control_model: &PtzControlModel) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::control_ptz(ptz_control_model).await?;
        SipRequestOutput::new(&ptz_control_model.deviceId, association, request)
            .send_log("control_ptz")
            .await;
        Ok(())
    }

    pub async fn snapshot_image(
        device_id: &String,
        channel_id: &String,
        num: u8,
        interval: u8,
        uri: &String,
        session_id: &String,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::control_snapshot_image(
            device_id, channel_id, num, interval, uri, session_id,
        )
        .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("snapshot_image")
            .await;
        Ok(())
    }
    pub async fn snapshot_image_call(
        device_id: &String,
        channel_id: &String,
        num: u8,
        interval: u8,
        uri: &String,
        session_id: &String,
    ) -> GlobalResult<Response> {
        let (request, association) = RequestBuilder::control_snapshot_image(
            device_id, channel_id, num, interval, uri, session_id,
        )
        .await?;
        let (tx,rx) = oneshot::channel();
        let cb = default_response_callback(tx);
        SipRequestOutput::new(device_id, association, request)
            .send(cb)
            .await?;
        rx.await.hand_log(|msg|error!("{msg}"))?
    }
}

pub struct CmdNotify;

pub struct CmdStream;

impl CmdStream {
    pub async fn download_invite(
        device_id: &String,
        channel_id: &String,
        dst_ip: &String,
        dst_port: u16,
        stream_mode: TransMode,
        ssrc: &String,
        st: u32,
        et: u32,
        speed: u8,
    ) -> GlobalResult<(Response, MediaExt, String, String, Association)> {
        let (request, association) = RequestBuilder::download(
            device_id,
            channel_id,
            dst_ip,
            dst_port,
            stream_mode,
            ssrc,
            st,
            et,
            speed,
        )
        .await
        .hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(device_id, association, request).await
    }

    pub async fn play_back_invite(
        device_id: &String,
        channel_id: &String,
        dst_ip: &String,
        dst_port: u16,
        stream_mode: TransMode,
        ssrc: &String,
        st: u32,
        et: u32,
    ) -> GlobalResult<(Response, MediaExt, String, String, Association)> {
        let (request, association) = RequestBuilder::playback(
            device_id,
            channel_id,
            dst_ip,
            dst_port,
            stream_mode,
            ssrc,
            st,
            et,
        )
        .await
        .hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(device_id, association, request).await
    }
    pub async fn play_live_invite(
        device_id: &String,
        channel_id: &String,
        dst_ip: &String,
        dst_port: u16,
        stream_mode: TransMode,
        ssrc: &String,
    ) -> GlobalResult<(Response, MediaExt, String, String, Association)> {
        let (request, association) = RequestBuilder::play_live_request(
            device_id,
            channel_id,
            dst_ip,
            dst_port,
            stream_mode,
            ssrc,
        )
        .await
        .hand_log(|msg| warn!("{msg}"))?;
        Self::invite_stream(device_id, association, request).await
    }

    pub async fn invite_ack(
        device_id: &String,
        response: &Response,
        association: Association,
    ) -> GlobalResult<(String, u32)> {
        let ack_request = RequestBuilder::build_ack_request_by_response(response,device_id)?;
        let call_id = ack_request.call_id()?.value().to_string();
        let seq = ack_request.seq()?;
        SipRequestOutput::new(device_id, association, ack_request)
            .send_log("invite_ack")
            .await;
        Ok((call_id, seq))
    }
    pub async fn play_speed(
        device_id: &String,
        channel_id: &String,
        speed: f32,
        from_tag: &str,
        to_tag: &str,
        seq: u32,
        call_id: String,
    ) -> GlobalResult<()> {
        let (request, association) =
            RequestBuilder::speed(device_id, channel_id, speed, from_tag, to_tag, seq, call_id)
                .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("play_speed")
            .await;
        Ok(())
    }
    pub async fn play_seek(
        device_id: &String,
        channel_id: &String,
        seek: u32,
        from_tag: &str,
        to_tag: &str,
        seq: u32,
        call_id: String,
    ) -> GlobalResult<()> {
        let (request, association) =
            RequestBuilder::seek(device_id, channel_id, seek, from_tag, to_tag, seq, call_id)
                .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("play_seek")
            .await;
        Ok(())
    }

    pub async fn play_bye(
        seq: u32,
        call_id: String,
        device_id: &String,
        channel_id: &String,
        from_tag: &str,
        to_tag: &str,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::build_bye_request(
            seq, call_id, device_id, channel_id, from_tag, to_tag,
        )
        .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("play_bye")
            .await;
        Ok(())
    }

    async fn invite_stream(
        device_id: &String,
        association: Association,
        request: Request,
    ) -> GlobalResult<(Response, MediaExt, String, String, Association)> {
        let (tx, rx) = oneshot::channel();
        let cb = default_response_callback(tx);
        SipRequestOutput::new(device_id, association.clone(), request)
            .send(cb)
            .await?;
        let res = rx.await.hand_log(|msg|error!("{msg}"))??;
        let code = res.status_code.code();
        let code_msg = res.status_code.to_string();
        if code >= 200 || code <= 299 {
            if let Ok(ext) = Self::parse_sdp(res.body()) {
                let from_tag = res.header_from_tag()?.to_string();
                let to_tag = res
                    .header_to_tag()?
                    .ok_or(SysErr(anyhow!("to tag is none")))?
                    .to_string();
                return Ok((res, ext, from_tag, to_tag, association));
            }
        }
        Err(GlobalError::new_biz_error(3000, &code_msg, |msg| {
            error!("{msg}")
        }))
    }

    fn parse_sdp(sdp: &Vec<u8>) -> GlobalResult<MediaExt> {
        let session = sdp_types::Session::parse(sdp).hand_log(|msg| error!("{msg}"))?;
        let re = Regex::new(r"\s+").hand_log(|msg| error!("{msg}"))?;
        let mut ext = MediaExt::default();
        for media in session.medias {
            if matches!(&*(media.media.trim().to_lowercase()), "video" | "audio") {
                if let Some(info) = media
                    .get_first_attribute_value("rtpmap")
                    .hand_log(|msg| error!("{msg}"))?
                {
                    let trimmed = re.replace_all(info, " ").trim().to_string();
                    if let Some((play_code, payload)) = trimmed.split_once(' ') {
                        let type_code: u8 =
                            play_code.trim().parse().hand_log(|msg| error!("{msg}"))?;
                        ext.type_code = type_code;
                        let vs: Vec<&str> = payload.trim().split('/').collect();
                        if vs.len() >= 2 {
                            ext.type_name = vs[0].trim().to_uppercase();
                            ext.clock_rate =
                                vs[1].trim().parse().hand_log(|msg| error!("{msg}"))?;
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
                if !parts[1].is_empty() {
                    me.video_params.map_video_codec(parts[1]);
                }
                if !parts[2].is_empty() {
                    me.video_params.map_resolution(parts[2]);
                }
                if !parts[3].is_empty() {
                    me.video_params.map_fps(parts[3]);
                }
                if !parts[4].is_empty() {
                    me.video_params.map_bitrate_type(parts[4]);
                }
                let p5 = parts[5].strip_suffix("a").unwrap().trim();
                if !p5.is_empty() {
                    me.video_params.map_bitrate(p5);
                }
                if !parts[6].is_empty() {
                    me.audio_params.map_audio_codec(parts[6]);
                }
                if !parts[7].is_empty() {
                    me.audio_params.map_bitrate(parts[7]);
                }
                if !parts[8].is_empty() {
                    me.audio_params.map_sample_rate(parts[8]);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(unused)]
mod test {
    use crate::gb::handler::cmd::CmdStream;
    use regex::Regex;

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
