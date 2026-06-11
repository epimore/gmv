//! PJSIP-backed GB28181 outbound business commands.
//!
//! This module replaces the old `gb::handler::cmd`/rsip layer. It only sends
//! SIP bytes produced by `gmv_pjsip` and keeps small business waiters for APIs
//! that need a synchronous result.

use std::time::Duration;

use base::bytes::Bytes;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::net::state::Protocol;
use gmv_pjsip::message::HeaderMapExt;
use gmv_pjsip::{SipMethod, parser::parse_sip_message};
use shared::info::media_info_ext::MediaExt;

use crate::gb::core::rw::RWContext;
use crate::register::core::Register;
use crate::state::model::{PtzControlModel, TransMode};
use crate::state::session::Cache as GeneralCache;

use super::adapter::{GbSipRuntime, pjsip_protocol_from_base};
use super::invite::{
    GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest, InviteTalkRequest,
};
use super::message::CreateDeviceMessageRequest;
use super::runtime_cache::{SipResponseKey, SipRuntimeCache, recv_with_timeout};
use super::{sdp, xml};

const INVITE_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const BYE_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_WAIT_TIMEOUT: Duration = Duration::from_secs(8);

fn runtime() -> GlobalResult<&'static GbSipRuntime> {
    GbSipRuntime::global().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "GB SIP runtime is not initialized",
            |msg| error!("{msg}"),
        )
    })
}

fn connected_target(device_id: &str) -> GlobalResult<(String, u16, Protocol)> {
    let Some(session) = Register::get_connected_device_session(device_id) else {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "device is not registered or connected",
            |msg| error!("device_id={device_id}; {msg}"),
        ));
    };
    Ok((
        session.association.remote_addr.ip().to_string(),
        session.association.remote_addr.port(),
        session.association.protocol,
    ))
}

async fn send_to_registered_device(device_id: &str, bytes: Bytes) -> GlobalResult<()> {
    let Some(session) = Register::get_connected_device_session(device_id) else {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "device is not registered or connected",
            |msg| error!("device_id={device_id}; {msg}"),
        ));
    };
    RWContext::send_sip_bytes(session.association, bytes).await
}

fn response_key(bytes: &Bytes) -> GlobalResult<SipResponseKey> {
    let message = parse_sip_message(bytes.clone()).map_err(to_global_error)?;
    let call_id = message.call_id().map_err(to_global_error)?;
    let cseq = message.cseq().map_err(to_global_error)?;
    Ok(SipResponseKey {
        method: SipMethod::parse(&cseq.method),
        call_id,
        cseq: cseq.number,
    })
}

async fn send_request_and_wait(device_id: &str, bytes: Bytes) -> GlobalResult<()> {
    let key = response_key(&bytes)?;
    let rx = SipRuntimeCache::global().insert_response_waiter(key.clone(), REQUEST_WAIT_TIMEOUT);
    if let Err(err) = send_to_registered_device(device_id, bytes).await {
        SipRuntimeCache::global().remove_response_waiter(&key);
        return Err(err);
    }
    let result = recv_with_timeout(rx, REQUEST_WAIT_TIMEOUT)
        .await
        .map_err(|reason| {
            SipRuntimeCache::global().remove_response_waiter(&key);
            GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device SIP response timeout",
                |msg| {
                    error!(
                        "device_id={device_id}; method={}; call_id={}; cseq={}; {msg}; reason={reason}",
                        key.method, key.call_id, key.cseq
                    )
                },
            )
        })?;
    if (200..300).contains(&result.status) {
        return Ok(());
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected SIP request",
        |msg| {
            error!(
                "device_id={device_id}; method={}; call_id={}; cseq={}; status={}; {msg}",
                key.method, key.call_id, key.cseq, result.status
            )
        },
    ))
}

pub async fn query_catalog(device_id: &str, sn: u32) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::catalog_query(
            device_id.to_string(),
            host,
            port,
            pjsip_protocol_from_base(proto),
            sn,
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn query_device_info(device_id: &str, sn: u32) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::device_info_query(
            device_id.to_string(),
            host,
            port,
            pjsip_protocol_from_base(proto),
            sn,
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn query_record_info(
    device_id: &str,
    sn: u32,
    start_time: &str,
    end_time: &str,
) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::record_info_query(
            device_id.to_string(),
            host,
            port,
            pjsip_protocol_from_base(proto),
            sn,
            start_time,
            end_time,
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn query_preset(device_id: &str) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::preset_query(
            device_id.to_string(),
            host,
            port,
            pjsip_protocol_from_base(proto),
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn send_xml_message(device_id: &str, body: String) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::xml(
            device_id.to_string(),
            host,
            port,
            pjsip_protocol_from_base(proto),
            body,
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn control_ptz(model: &PtzControlModel) -> GlobalResult<()> {
    let sn = base::chrono::Local::now()
        .timestamp()
        .unsigned_abs()
        .min(u64::from(u32::MAX)) as u32;
    let command = build_ptz_command(model)?;
    let body = xml::build_ptz_control(sn, &model.channelId, &command);
    send_xml_message(&model.deviceId, body).await
}

fn build_ptz_command(model: &PtzControlModel) -> GlobalResult<String> {
    if model.leftRight > 2 || model.upDown > 2 || model.inOut > 2 || model.zoomSpeed > 15 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "invalid PTZ control parameter",
            |msg| {
                error!(
                    "{msg}: left_right={}, up_down={}, in_out={}, zoom_speed={}",
                    model.leftRight, model.upDown, model.inOut, model.zoomSpeed
                )
            },
        ));
    }

    let mut bytes = [0xA5, 0x0F, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
    bytes[3] |= match model.leftRight {
        1 => 0x02,
        2 => 0x01,
        _ => 0,
    };
    bytes[3] |= match model.upDown {
        1 => 0x08,
        2 => 0x04,
        _ => 0,
    };
    bytes[3] |= match model.inOut {
        1 => 0x20,
        2 => 0x10,
        _ => 0,
    };
    bytes[4] = model.horizonSpeed;
    bytes[5] = model.verticalSpeed;
    bytes[6] = model.zoomSpeed << 4;
    bytes[7] = (bytes.iter().map(|value| u16::from(*value)).sum::<u16>() % 256) as u8;
    Ok(bytes.iter().map(|value| format!("{value:02X}")).collect())
}

pub async fn snapshot_image_call(
    device_id: &str,
    channel_id: &str,
    count: u8,
    interval: u8,
    url: &str,
    session_id: &str,
) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    let bytes = runtime()?
        .create_device_message(CreateDeviceMessageRequest::snapshot_control(
            device_id.to_string(),
            channel_id,
            host,
            port,
            pjsip_protocol_from_base(proto),
            count,
            interval,
            url,
            session_id,
        ))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn invite_play(req: InvitePlayRequest) -> GlobalResult<()> {
    let device_id = req.device_id.clone();
    let bytes = runtime()?
        .create_invite_play(req)
        .map_err(to_global_error)?;
    send_to_registered_device(&device_id, bytes).await
}

pub async fn invite_play_and_wait(req: InvitePlayRequest) -> GlobalResult<GbInviteAcceptedEvent> {
    let device_id = req.device_id.clone();
    let stream_id = req.stream_id.clone();
    let rx = SipRuntimeCache::global().insert_invite_waiter(stream_id.clone(), INVITE_WAIT_TIMEOUT);
    let bytes = match runtime()?.create_invite_play(req).map_err(to_global_error) {
        Ok(bytes) => bytes,
        Err(err) => {
            SipRuntimeCache::global().remove_stream_indexes(&stream_id, None);
            return Err(err);
        }
    };
    if let Err(err) = send_to_registered_device(&device_id, bytes).await {
        SipRuntimeCache::global().remove_stream_indexes(&stream_id, None);
        return Err(err);
    }
    match recv_with_timeout(rx, INVITE_WAIT_TIMEOUT).await {
        Ok(Ok(event)) => Ok(event),
        Ok(Err(failure)) => {
            SipRuntimeCache::global().remove_stream_indexes(&stream_id, Some(&failure.call_id));
            Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "device rejected INVITE",
                |msg| {
                    error!(
                        "stream_id={}; call_id={}; status={}; {msg}",
                        failure.stream_id, failure.call_id, failure.status
                    )
                },
            ))
        }
        Err(reason) => {
            SipRuntimeCache::global().remove_stream_indexes(&stream_id, None);
            Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device INVITE response timeout",
                |msg| error!("stream_id={stream_id}; {msg}; reason={reason}"),
            ))
        }
    }
}

pub async fn talk_invite_and_wait(req: InviteTalkRequest) -> GlobalResult<GbInviteAcceptedEvent> {
    let device_id = req.device_id.clone();
    let talk_id = req.talk_id.clone();
    let rx = SipRuntimeCache::global().insert_invite_waiter(talk_id.clone(), INVITE_WAIT_TIMEOUT);
    let bytes = match runtime()?.create_talk_invite(req).map_err(to_global_error) {
        Ok(bytes) => bytes,
        Err(err) => {
            SipRuntimeCache::global().remove_stream_indexes(&talk_id, None);
            return Err(err);
        }
    };
    if let Err(err) = send_to_registered_device(&device_id, bytes).await {
        SipRuntimeCache::global().remove_stream_indexes(&talk_id, None);
        return Err(err);
    }
    match recv_with_timeout(rx, INVITE_WAIT_TIMEOUT).await {
        Ok(Ok(event)) => Ok(event),
        Ok(Err(failure)) => {
            SipRuntimeCache::global().remove_stream_indexes(&talk_id, Some(&failure.call_id));
            Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "device rejected talk INVITE",
                |msg| {
                    error!(
                        "talk_id={}; call_id={}; status={}; {msg}",
                        failure.stream_id, failure.call_id, failure.status
                    )
                },
            ))
        }
        Err(reason) => {
            SipRuntimeCache::global().remove_stream_indexes(&talk_id, None);
            Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device talk INVITE response timeout",
                |msg| error!("talk_id={talk_id}; {msg}; reason={reason}"),
            ))
        }
    }
}

pub async fn play_live_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    trans_mode: TransMode,
    ssrc: &str,
    stream_id: &str,
) -> GlobalResult<(GbInviteAcceptedEvent, MediaExt)> {
    let (host, port, proto) = connected_target(device_id)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::play_live(channel_id, media_ip, media_port, trans_mode, ssrc, true);
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        device_host: host,
        device_port: port,
        media_ip: media_ip.to_string(),
        media_port,
        ssrc: ssrc_u32,
        payload_type: 96,
        protocol,
        sdp: Some(sdp),
        call_id: None,
        cseq: None,
        subject: None,
    })
    .await?;
    let ext = sdp::parse_media_ext(accepted.remote_sdp.as_bytes())?;
    Ok((accepted, ext))
}

pub async fn play_back_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    trans_mode: TransMode,
    ssrc: &str,
    stream_id: &str,
    st: u32,
    et: u32,
) -> GlobalResult<(GbInviteAcceptedEvent, MediaExt)> {
    let (host, port, proto) = connected_target(device_id)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::playback(
        channel_id, media_ip, media_port, trans_mode, ssrc, st, et, true,
    );
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        device_host: host,
        device_port: port,
        media_ip: media_ip.to_string(),
        media_port,
        ssrc: ssrc_u32,
        payload_type: 96,
        protocol,
        sdp: Some(sdp),
        call_id: None,
        cseq: None,
        subject: None,
    })
    .await?;
    let ext = sdp::parse_media_ext(accepted.remote_sdp.as_bytes())?;
    Ok((accepted, ext))
}

pub async fn download_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    trans_mode: TransMode,
    ssrc: &str,
    stream_id: &str,
    st: u32,
    et: u32,
    speed: u8,
) -> GlobalResult<(GbInviteAcceptedEvent, MediaExt)> {
    let (host, port, proto) = connected_target(device_id)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::download(
        channel_id, media_ip, media_port, trans_mode, ssrc, st, et, speed, true,
    );
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        device_host: host,
        device_port: port,
        media_ip: media_ip.to_string(),
        media_port,
        ssrc: ssrc_u32,
        payload_type: 96,
        protocol,
        sdp: Some(sdp),
        call_id: None,
        cseq: None,
        subject: None,
    })
    .await?;
    let ext = sdp::parse_media_ext(accepted.remote_sdp.as_bytes())?;
    Ok((accepted, ext))
}

pub async fn invite_stop_by_device(device_id: &str, req: InviteStopRequest) -> GlobalResult<()> {
    let key = req
        .call_id
        .clone()
        .or_else(|| req.stream_id.clone())
        .unwrap_or_else(|| device_id.to_string());
    let rx = SipRuntimeCache::global().insert_bye_waiter(key.clone(), BYE_WAIT_TIMEOUT);
    let bytes = match runtime()?.create_invite_stop(req).map_err(to_global_error) {
        Ok(bytes) => bytes,
        Err(err) => {
            SipRuntimeCache::global().remove_bye_waiter(&key);
            return Err(err);
        }
    };
    if let Err(err) = send_to_registered_device(device_id, bytes).await {
        SipRuntimeCache::global().remove_bye_waiter(&key);
        return Err(err);
    }
    match recv_with_timeout(rx, BYE_WAIT_TIMEOUT).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(failure)) => Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "device rejected BYE",
            |msg| {
                error!(
                    "device_id={device_id}; call_id={}; status={}; {msg}",
                    failure.call_id, failure.status
                )
            },
        )),
        Err(reason) => {
            SipRuntimeCache::global().remove_bye_waiter(&key);
            Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device BYE response timeout",
                |msg| error!("device_id={device_id}; key={key}; {msg}; reason={reason}"),
            ))
        }
    }
}

pub async fn invite_stop_by_stream(stream_id: &str) -> GlobalResult<()> {
    let Some(command) = GeneralCache::stream_dialog_next(stream_id) else {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "流不存在",
            |msg| error!("{msg}"),
        ));
    };
    let Some(device_id) = GeneralCache::stream_device_id(stream_id) else {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "流设备状态不存在",
            |msg| error!("{msg}"),
        ));
    };
    invite_stop_by_device(
        &device_id,
        InviteStopRequest {
            call_id: Some(command.call_id),
            stream_id: Some(stream_id.to_string()),
        },
    )
    .await
}

pub async fn play_seek(device_id: &str, stream_id: &str, seek_second: u32) -> GlobalResult<()> {
    let bytes = runtime()?
        .create_playback_seek_info(stream_id, f64::from(seek_second))
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

pub async fn play_speed(device_id: &str, stream_id: &str, speed: f32) -> GlobalResult<()> {
    let bytes = runtime()?
        .create_playback_speed_info(stream_id, speed)
        .map_err(to_global_error)?;
    send_request_and_wait(device_id, bytes).await
}

#[cfg(test)]
mod tests {
    use super::build_ptz_command;
    use crate::state::model::PtzControlModel;

    #[test]
    fn builds_gb28181_ptz_hex_command() {
        let model = PtzControlModel {
            deviceId: "34020000001320000001".to_string(),
            channelId: "34020000001320000002".to_string(),
            leftRight: 1,
            upDown: 1,
            inOut: 2,
            horizonSpeed: 32,
            verticalSpeed: 16,
            zoomSpeed: 3,
        };

        assert_eq!(build_ptz_command(&model).unwrap(), "A50F011A2010302F");
    }
}

pub async fn refresh_catalog_subscription(device_id: &str, generation: u64) -> GlobalResult<()> {
    query_catalog(device_id, generation.min(u64::from(u32::MAX)) as u32).await
}

pub fn transport_protocol(
    trans_mode: TransMode,
    fallback: Protocol,
) -> gmv_pjsip::SipTransportProtocol {
    match trans_mode {
        TransMode::TcpActive | TransMode::TcpPassive => gmv_pjsip::SipTransportProtocol::Tcp,
        TransMode::Udp if matches!(fallback, Protocol::TCP) => gmv_pjsip::SipTransportProtocol::Tcp,
        TransMode::Udp => gmv_pjsip::SipTransportProtocol::Udp,
    }
}

pub fn to_global_error(err: gmv_pjsip::SipError) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        &format!("pjsip operation failed: {err}"),
        |msg| error!("{msg}"),
    )
}
