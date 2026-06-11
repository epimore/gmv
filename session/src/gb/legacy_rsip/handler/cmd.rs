//! LEGACY RSIP PIPELINE
//!
//! The medium-term SIP stack has moved to `crate::gb::sip` + `gmv_pjsip`.
//! This file is kept temporarily for compatibility with existing service APIs
//! and for migration reference. New code must not add SIP parsing, transaction,
//! dialog, CSeq/tag/branch, or header-generation logic here.
//!

use crate::gb::core::rw::SipRequestOutput;
use crate::gb::depot::{Callback, default_response_callback};
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::handler::builder::{DialogTarget, RequestBuilder};
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::model::{PtzControlModel, TransMode};
use crate::state::session::{Cache, DialogCommand};
use anyhow::anyhow;
use base::err::BaseErrorCode;
use base::exception::GlobalError::SysErr;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::net::state::{Association, Protocol};
use base::tokio::sync::oneshot;
use base::tokio::time::{Duration, sleep};
use regex::Regex;
use rsip::message::HeadersExt;
use rsip::prelude::UntypedHeader;
use rsip::{Header, Request, Response};
use shared::info::media_info_ext::MediaExt;
use std::sync::Arc;

pub struct CmdResponse;

pub struct CmdQuery;

impl CmdQuery {
    fn catalog_refresh_delay(expires: u32) -> Duration {
        let margin = (expires / 10).clamp(1, 60);
        Duration::from_secs(u64::from(expires.saturating_sub(margin).max(1)))
    }

    fn schedule_catalog_refresh(device_id: Arc<str>, generation: u64, expires: u32) {
        let key = TimeScheduleKey::CatalogSubscription(device_id, generation);
        let _ = Register::scheduler().remove_register(&key);
        if let Err(err) =
            Register::scheduler().insert_register(key, Self::catalog_refresh_delay(expires))
        {
            warn!("schedule catalog subscription refresh failed: {err}");
        }
    }

    fn schedule_catalog_retry(device_id: Arc<str>, generation: u64, expires: u32) {
        let key = TimeScheduleKey::CatalogSubscription(device_id, generation);
        let _ = Register::scheduler().remove_register(&key);
        let delay = Duration::from_secs(u64::from(expires.clamp(1, 30)));
        if let Err(err) = Register::scheduler().insert_register(key, delay) {
            warn!("schedule catalog subscription retry failed: {err}");
        }
    }

    pub fn update_catalog_subscription_from_notify(
        device_id: &str,
        generation: u64,
        expires: u32,
    ) {
        if Cache::catalog_subscription_update_expires(device_id, generation, expires) {
            Self::schedule_catalog_refresh(Arc::from(device_id), generation, expires);
        }
    }

    pub fn terminate_catalog_subscription(device_id: &str, generation: u64) {
        let expires =
            Cache::catalog_subscription_expires(device_id, generation);
        if Cache::catalog_subscription_remove(device_id, Some(generation)) {
            if let Some(expires) = expires {
                Self::retry_new_catalog_subscription(device_id.to_string(), expires);
            }
        }
    }

    fn retry_new_catalog_subscription(device_id: String, expires: u32) {
        base::tokio::spawn(async move {
            let mut delay = Duration::from_secs(5);
            loop {
                sleep(delay).await;
                if !crate::gb::core::rw::RWContext::has_session_by_device_id(
                    &device_id,
                ) {
                    break;
                }
                match Self::subscribe_device_catalog(&device_id, expires).await {
                    Ok(()) => break,
                    Err(err) => {
                        warn!("retry catalog subscription failed: {err}");
                        delay = Duration::from_secs(30);
                    }
                }
            }
        });
    }

    fn event_header(request: &Request) -> GlobalResult<String> {
        request
            .headers
            .iter()
            .find_map(|header| match header {
                Header::Event(event) => Some(event.value().to_string()),
                _ => None,
            })
            .ok_or_else(|| {
                GlobalError::new_sys_error("SUBSCRIBE missing Event header", |msg| {
                    error!("{msg}")
                })
            })
    }

    fn response_expires(response: &Response, default: u32) -> u32 {
        response
            .headers
            .iter()
            .find_map(|header| match header {
                Header::Expires(expires) => expires.value().parse::<u32>().ok(),
                _ => None,
            })
            .unwrap_or(default)
    }

    fn complete_catalog_subscription(
        device_id: &str,
        generation: u64,
        fallback_remote_target: &str,
        requested_expires: u32,
        response: &Response,
    ) -> GlobalResult<u32> {
        let target =
            RequestBuilder::dialog_target_by_response(response, fallback_remote_target);
        let remote_tag = response
            .header_to_tag()?
            .ok_or_else(|| {
                GlobalError::new_sys_error(
                    "SUBSCRIBE final response missing To tag",
                    |msg| error!("{msg}"),
                )
            })?
            .to_string();
        if !Cache::catalog_subscription_complete(
            device_id,
            generation,
            target.remote_target,
            target.route_set,
            target.from_header,
            target.to_header,
            remote_tag,
        ) {
            return Err(GlobalError::new_sys_error(
                "catalog subscription state changed before response",
                |msg| warn!("device_id={device_id}; {msg}"),
            ));
        }
        let expires = Self::response_expires(response, requested_expires);
        Cache::catalog_subscription_update_expires(device_id, generation, expires);
        Ok(expires)
    }

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
        let call_id = request.call_id()?.value().to_string();
        let seq = request.seq()?;
        let event = Self::event_header(&request)?;
        let remote_target = request.uri.to_string();
        let from_header = request
            .from_header()
            .hand_log(|msg| warn!("{msg}"))?
            .value()
            .to_string();
        let to_header = request
            .to_header()
            .hand_log(|msg| warn!("{msg}"))?
            .value()
            .to_string();
        let local_tag = request.header_from_tag()?.to_string();
        let Some(generation) = Cache::catalog_subscription_begin(
            device_id.clone(),
            call_id,
            seq,
            event,
            expire,
            remote_target.clone(),
            from_header,
            to_header,
            local_tag,
        ) else {
            return Ok(());
        };
        let callback_device_id = device_id.clone();
        let schedule_device_id: Arc<str> = Arc::from(device_id.as_str());
        let callback: Callback = Box::new(move |result| match result {
            Ok(response) if (200..300).contains(&response.status_code.code()) => {
                match Self::complete_catalog_subscription(
                    &callback_device_id,
                    generation,
                    &remote_target,
                    expire,
                    &response,
                ) {
                    Ok(expires) => Self::schedule_catalog_refresh(
                        schedule_device_id,
                        generation,
                        expires,
                    ),
                    Err(err) => {
                        Cache::catalog_subscription_remove(
                            &callback_device_id,
                            Some(generation),
                        );
                        warn!("complete catalog subscription failed: {err}");
                    }
                }
            }
            Ok(response) => {
                Cache::catalog_subscription_remove(&callback_device_id, Some(generation));
                warn!(
                    "catalog subscription rejected: device_id={}, status={}",
                    callback_device_id,
                    response.status_code.code()
                );
                Self::retry_new_catalog_subscription(callback_device_id, expire);
            }
            Err(err) => {
                Cache::catalog_subscription_remove(&callback_device_id, Some(generation));
                warn!(
                    "catalog subscription request failed: device_id={}, err={}",
                    callback_device_id, err
                );
                Self::retry_new_catalog_subscription(callback_device_id, expire);
            }
        });
        if let Err(err) = SipRequestOutput::new(device_id, association, request)
            .send(callback)
            .await
        {
            Cache::catalog_subscription_remove(device_id, Some(generation));
            return Err(err);
        }
        Ok(())
    }

    pub async fn refresh_catalog_subscription(
        device_id: Arc<str>,
        generation: u64,
    ) -> GlobalResult<()> {
        let Some(command) =
            Cache::catalog_subscription_take_refresh(device_id.as_ref(), generation)
        else {
            return Ok(());
        };
        let device_key = device_id.to_string();
        let (request, association) =
            match RequestBuilder::refresh_device_catalog_subscription(&device_key, &command) {
                Ok(request) => request,
                Err(err) => {
                    Cache::catalog_subscription_mark_failed(&device_key, generation);
                    Self::schedule_catalog_retry(device_id, generation, command.expires);
                    return Err(err);
                }
            };
        let remote_target = command.remote_target.clone();
        let requested_expires = command.expires;
        let callback_device_id = device_key.clone();
        let schedule_device_id = device_id.clone();
        let callback: Callback = Box::new(move |result| match result {
            Ok(response) if (200..300).contains(&response.status_code.code()) => {
                match Self::complete_catalog_subscription(
                    &callback_device_id,
                    generation,
                    &remote_target,
                    requested_expires,
                    &response,
                ) {
                    Ok(expires) => Self::schedule_catalog_refresh(
                        schedule_device_id,
                        generation,
                        expires,
                    ),
                    Err(err) => warn!("complete catalog refresh failed: {err}"),
                }
            }
            Ok(response) if response.status_code.code() == 481 => {
                Cache::catalog_subscription_remove(&callback_device_id, Some(generation));
                Self::retry_new_catalog_subscription(
                    callback_device_id,
                    requested_expires,
                );
            }
            Ok(response) => {
                Cache::catalog_subscription_mark_failed(&callback_device_id, generation);
                warn!(
                    "catalog subscription refresh rejected: device_id={}, status={}",
                    callback_device_id,
                    response.status_code.code()
                );
                Self::schedule_catalog_retry(
                    schedule_device_id,
                    generation,
                    requested_expires,
                );
            }
            Err(err) => {
                Cache::catalog_subscription_mark_failed(&callback_device_id, generation);
                warn!(
                    "catalog subscription refresh failed: device_id={}, err={}",
                    callback_device_id, err
                );
                Self::schedule_catalog_retry(
                    schedule_device_id,
                    generation,
                    requested_expires,
                );
            }
        });
        if let Err(err) = SipRequestOutput::new(&device_key, association, request)
            .send(callback)
            .await
        {
            Cache::catalog_subscription_mark_failed(&device_key, generation);
            Self::schedule_catalog_retry(device_id, generation, requested_expires);
            return Err(err);
        }
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
        let (tx, rx) = oneshot::channel();
        let cb = default_response_callback(tx);
        SipRequestOutput::new(device_id, association, request)
            .send(cb)
            .await?;
        rx.await.hand_log(|msg| error!("{msg}"))?
    }
}

pub struct CmdNotify;

pub struct TalkSdpAnswer {
    pub device_ip: String,
    pub device_port: u16,
    pub protocol: Protocol,
    pub payload_type: u8,
    pub codec: String,
    pub sample_rate: u32,
}

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

    pub async fn talk_invite(
        device_id: &String,
        channel_id: &String,
        dst_ip: &String,
        dst_port: u16,
        stream_mode: TransMode,
        ssrc: &String,
        payload_type: u8,
        codec: &str,
        sample_rate: u32,
    ) -> GlobalResult<(Response, TalkSdpAnswer, String, String, Association)> {
        let (request, association) = RequestBuilder::talk(
            device_id,
            channel_id,
            dst_ip,
            dst_port,
            stream_mode,
            ssrc,
            payload_type,
            codec,
            sample_rate,
        )
        .await
        .hand_log(|msg| warn!("{msg}"))?;
        Self::invite_talk(device_id, association, request).await
    }

    pub async fn invite_ack(
        device_id: &String,
        response: &Response,
        association: Association,
    ) -> GlobalResult<(String, u32)> {
        let (call_id, seq, _) =
            Self::invite_ack_with_dialog(device_id, response, association).await?;
        Ok((call_id, seq))
    }

    pub async fn invite_ack_with_dialog(
        device_id: &String,
        response: &Response,
        association: Association,
    ) -> GlobalResult<(String, u32, DialogTarget)> {
        let (ack_request, dialog_target) =
            RequestBuilder::build_ack_request_and_dialog_by_response(response, device_id)?;
        let call_id = ack_request.call_id()?.value().to_string();
        let seq = ack_request.seq()?;
        SipRequestOutput::new(device_id, association, ack_request)
            .send_log("invite_ack")
            .await;
        Ok((call_id, seq, dialog_target))
    }
    pub async fn play_speed(
        device_id: &String,
        speed: f32,
        command: DialogCommand,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::speed(
            device_id,
            speed,
            &command.remote_target,
            &command.route_set,
            &command.from_header,
            &command.to_header,
            command.seq,
            command.call_id,
        )
        .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("play_speed")
            .await;
        Ok(())
    }
    pub async fn play_seek(
        device_id: &String,
        seek: u32,
        command: DialogCommand,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::seek(
            device_id,
            seek,
            &command.remote_target,
            &command.route_set,
            &command.from_header,
            &command.to_header,
            command.seq,
            command.call_id,
        )
        .await?;
        SipRequestOutput::new(device_id, association, request)
            .send_log("play_seek")
            .await;
        Ok(())
    }

    pub async fn play_bye_with_callback(
        seq: u32,
        call_id: String,
        device_id: &String,
        remote_target: &str,
        route_set: &[String],
        from_header: &str,
        to_header: &str,
        callback: Callback,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::build_dialog_bye_request(
            seq,
            call_id,
            device_id,
            remote_target,
            route_set,
            from_header,
            to_header,
        )?;
        SipRequestOutput::new(device_id, association, request)
            .send(callback)
            .await
    }

    pub async fn play_bye_dialog(
        seq: u32,
        call_id: String,
        device_id: &String,
        remote_target: &str,
        route_set: &[String],
        from_header: &str,
        to_header: &str,
    ) -> GlobalResult<()> {
        let (request, association) = RequestBuilder::build_dialog_bye_request(
            seq,
            call_id,
            device_id,
            remote_target,
            route_set,
            from_header,
            to_header,
        )?;
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
        let res = rx.await.hand_log(|msg| error!("{msg}"))??;
        let code = res.status_code.code();
        let code_msg = res.status_code.to_string();
        if code >= 200 && code <= 299 {
            if let Ok(ext) = Self::parse_sdp(res.body()) {
                let from_tag = res.header_from_tag()?.to_string();
                let to_tag = res
                    .header_to_tag()?
                    .ok_or(SysErr(anyhow!("to tag is none")))?
                    .to_string();
                return Ok((res, ext, from_tag, to_tag, association));
            }
        }
        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            &code_msg,
            |msg| error!("{msg}"),
        ))
    }

    async fn invite_talk(
        device_id: &String,
        association: Association,
        request: Request,
    ) -> GlobalResult<(Response, TalkSdpAnswer, String, String, Association)> {
        let (tx, rx) = oneshot::channel();
        let cb = default_response_callback(tx);
        SipRequestOutput::new(device_id, association.clone(), request)
            .send(cb)
            .await?;
        let res = rx.await.hand_log(|msg| error!("{msg}"))??;
        let code = res.status_code.code();
        let code_msg = res.status_code.to_string();
        if code >= 200 && code <= 299 {
            let answer = Self::parse_talk_sdp(res.body())?;
            let from_tag = res.header_from_tag()?.to_string();
            let to_tag = res
                .header_to_tag()?
                .ok_or(SysErr(anyhow!("to tag is none")))?
                .to_string();
            return Ok((res, answer, from_tag, to_tag, association));
        }
        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            &code_msg,
            |msg| error!("{msg}"),
        ))
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

    fn parse_talk_sdp(sdp: &Vec<u8>) -> GlobalResult<TalkSdpAnswer> {
        let session = sdp_types::Session::parse(sdp).hand_log(|msg| error!("{msg}"))?;
        let re = Regex::new(r"\s+").hand_log(|msg| error!("{msg}"))?;
        let protocol = Self::parse_talk_sdp_protocol(sdp);
        let session_ip = session
            .connection
            .as_ref()
            .map(|conn| conn.connection_address.clone());

        for media in session.medias {
            if !media.media.trim().eq_ignore_ascii_case("audio") {
                continue;
            }

            let device_ip = media
                .connections
                .first()
                .map(|conn| conn.connection_address.clone())
                .or_else(|| session_ip.clone())
                .ok_or_else(|| {
                    GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "talk sdp missing audio connection address",
                        |msg| error!("{msg}"),
                    )
                })?;
            let mut payload_type = media
                .fmt
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(8);
            let mut codec = match payload_type {
                0 => "PCMU".to_string(),
                8 => "PCMA".to_string(),
                _ => String::new(),
            };
            let mut sample_rate = 8000u32;

            if let Ok(Some(info)) = media.get_first_attribute_value("rtpmap") {
                let trimmed = re.replace_all(info, " ").trim().to_string();
                if let Some((play_code, payload)) = trimmed.split_once(' ') {
                    payload_type = play_code.trim().parse().hand_log(|msg| error!("{msg}"))?;
                    let parts = payload.trim().split('/').collect::<Vec<_>>();
                    if parts.len() >= 2 {
                        codec = parts[0].trim().to_uppercase();
                        sample_rate = parts[1].trim().parse().hand_log(|msg| error!("{msg}"))?;
                    }
                }
            }

            if codec.is_empty() {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::Unsupported.code(),
                    "unsupported talk payload type",
                    |msg| error!("{msg}: pt={payload_type}"),
                ));
            }

            return Ok(TalkSdpAnswer {
                device_ip,
                device_port: media.port,
                protocol,
                payload_type,
                codec,
                sample_rate,
            });
        }

        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "talk sdp missing audio media",
            |msg| error!("{msg}"),
        ))
    }

    fn parse_talk_sdp_protocol(sdp: &[u8]) -> Protocol {
        let text = String::from_utf8_lossy(sdp);
        for line in text.lines() {
            let line = line.trim();
            if !line
                .get(..7)
                .map(|prefix| prefix.eq_ignore_ascii_case("m=audio"))
                .unwrap_or(false)
            {
                continue;
            }
            let mut parts = line.split_whitespace();
            let _media = parts.next();
            let _port = parts.next();
            let transport = parts.next().unwrap_or_default();
            if transport.to_ascii_uppercase().contains("TCP") {
                return Protocol::TCP;
            }
            return Protocol::UDP;
        }
        Protocol::UDP
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
