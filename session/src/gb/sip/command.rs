//! PJSIP-backed GB28181 outbound business commands.
//!
//! This module replaces the old `gb::handler::cmd`/rsip layer. It only sends
//! SIP bytes produced by `gmv_pjsip` and keeps small business waiters for APIs
//! that need a synchronous result.

use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::net::state::Protocol;
use gmv_pjsip::gb28181::sdp::{TalkSdpOptions, build_play_sdp, build_talk_sdp};
use gmv_pjsip::gb28181::xml::{
    CONTENT_TYPE_MANSRTSP, build_mansrtsp_seek_body, build_mansrtsp_speed_body,
};
use gmv_pjsip::{
    SipDialogMethod, SipDialogRequest, SipDialogSnapshot, SipInviteIdentity, SipOutboundInvite,
    SipOutboundMessage, SipRestoredDialogRequest,
};
use shared::info::media_info_ext::MediaExt;

use crate::gb::SessionConf;
use crate::register::core::Register;
use crate::state::model::{PtzControlModel, TransMode};
use crate::state::session::Cache as GeneralCache;
use crate::storage::dialog_session::{
    DialogSessionType, DialogState, DialogTransport, EstablishedDialogFields, SipDialogSession,
    SipDialogSessionRepository,
};

use super::adapter::pjsip_protocol_from_base;
use super::invite::{
    GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest, InviteTalkRequest,
};
use super::message::{CreateDeviceMessageRequest, target_uri};
use super::native_runtime::NativeSipRuntimeHandle;
use super::runtime_cache::{NativeInviteMetadata, SipRuntimeCache, recv_with_timeout};
use super::{sdp, xml};

const INVITE_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const BYE_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const DIALOG_EXPIRE_HOURS: i64 = 8;

struct DurableDialogReservation {
    signal_node_id: String,
    version: i64,
}

pub(super) fn connected_target(device_id: &str) -> GlobalResult<(String, u16, Protocol)> {
    let Some(session) = Register::get_connected_device_session(device_id) else {
        return Err(device_not_connected(device_id));
    };
    Ok((
        session.association.remote_addr.ip().to_string(),
        session.association.remote_addr.port(),
        session.association.protocol,
    ))
}

async fn send_native_message_and_wait(request: CreateDeviceMessageRequest) -> GlobalResult<()> {
    let device_id = request.device_id.clone();
    let Some(session) = Register::get_connected_device_session(&device_id) else {
        return Err(device_not_connected(&device_id));
    };
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx =
        SipRuntimeCache::global().insert_native_response_waiter(operation_id, REQUEST_WAIT_TIMEOUT);
    let conf = SessionConf::get_session_by_conf();
    let message = SipOutboundMessage {
        operation_id,
        association_id: 0,
        protocol: request.protocol,
        target_uri: request.target_uri(),
        from_uri: format!("<sip:{}@{}:{}>", conf.domain_id, conf.wan_ip, conf.wan_port),
        content_type: request.content_type,
        body: request.body.to_vec(),
    };
    if let Err(err) = runtime.send_message(&session.association, message) {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        return Err(err);
    }
    let result = recv_with_timeout(rx, REQUEST_WAIT_TIMEOUT)
        .await
        .map_err(|reason| {
            SipRuntimeCache::global().remove_native_response_waiter(operation_id);
            GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device SIP response timeout",
                |msg| {
                    error!(
                        "device_id={device_id}; operation_id={operation_id}; {msg}; reason={reason}"
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
                "device_id={device_id}; operation_id={operation_id}; status={}; {msg}",
                result.status
            )
        },
    ))
}

async fn send_native_dialog_and_wait(
    device_id: &str,
    method: SipDialogMethod,
    call_id: String,
    content_type: Option<String>,
    body: Vec<u8>,
    timeout: Duration,
) -> GlobalResult<()> {
    if Register::get_connected_device_session(device_id).is_none() {
        return Err(device_not_connected(device_id));
    }
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global().insert_native_response_waiter(operation_id, timeout);
    if let Err(err) = runtime.send_dialog_request(SipDialogRequest {
        operation_id,
        method,
        call_id: call_id.clone(),
        content_type,
        body,
    }) {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        return Err(err);
    }
    let result = recv_with_timeout(rx, timeout).await.map_err(|reason| {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        GlobalError::new_biz_error(
            BaseErrorCode::Timeout.code(),
            "device SIP dialog response timeout",
            |msg| {
                error!(
                    "device_id={device_id}; call_id={call_id}; operation_id={operation_id}; \
                     {msg}; reason={reason}"
                )
            },
        )
    })?;
    if (200..300).contains(&result.status)
        || (method == SipDialogMethod::Bye && result.status == 481)
    {
        return Ok(());
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected SIP dialog request",
        |msg| {
            error!(
                "device_id={device_id}; call_id={call_id}; operation_id={operation_id}; \
                 status={}; {msg}",
                result.status
            )
        },
    ))
}

async fn send_restored_dialog_and_wait(
    device_id: &str,
    method: SipDialogMethod,
    session: &SipDialogSession,
    content_type: Option<String>,
    body: Vec<u8>,
    timeout: Duration,
) -> GlobalResult<()> {
    let association = Register::get_connected_device_session(device_id);
    if session.transport == DialogTransport::Tcp && association.is_none() {
        return Err(device_not_connected(device_id));
    }
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global().insert_native_response_waiter(operation_id, timeout);
    let request = SipRestoredDialogRequest {
        operation_id,
        method,
        snapshot: dialog_snapshot_from_session(session)?,
        content_type,
        body,
    };
    if let Err(err) = runtime.send_restored_dialog_request(
        association.as_ref().map(|value| &value.association),
        request,
    ) {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        return Err(err);
    }
    let result = recv_with_timeout(rx, timeout).await.map_err(|reason| {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        GlobalError::new_biz_error(
            BaseErrorCode::Timeout.code(),
            "device restored SIP dialog response timeout",
            |msg| {
                error!(
                    "device_id={device_id}; call_id={}; operation_id={operation_id}; \
                     {msg}; reason={reason}",
                    session.call_id
                )
            },
        )
    })?;
    if (200..300).contains(&result.status)
        || (method == SipDialogMethod::Bye && result.status == 481)
    {
        return Ok(());
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected restored SIP dialog request",
        |msg| {
            error!(
                "device_id={device_id}; call_id={}; operation_id={operation_id}; \
                 status={}; {msg}",
                session.call_id, result.status
            )
        },
    ))
}

fn dialog_snapshot_from_session(session: &SipDialogSession) -> GlobalResult<SipDialogSnapshot> {
    let remote_tag = session.remote_tag.clone().ok_or_else(|| {
        GlobalError::new_sys_error("restored SIP dialog remote tag is missing", |msg| {
            error!("stream_id={}; {msg}", session.stream_id)
        })
    })?;
    let remote_target = session
        .contact_uri
        .clone()
        .unwrap_or_else(|| session.remote_uri.clone());
    Ok(SipDialogSnapshot {
        call_id: session.call_id.clone(),
        local_uri: session.local_uri.clone(),
        remote_uri: session.remote_uri.clone(),
        local_tag: session.local_tag.clone(),
        remote_tag,
        local_cseq: u32::try_from(session.local_cseq).map_err(|_| {
            GlobalError::new_sys_error("restored SIP dialog CSeq is out of range", |msg| {
                error!("stream_id={}; {msg}", session.stream_id)
            })
        })?,
        remote_target,
        route_set: session.route_set.clone(),
        protocol: match session.transport {
            DialogTransport::Udp => gmv_pjsip::SipTransportProtocol::Udp,
            DialogTransport::Tcp => gmv_pjsip::SipTransportProtocol::Tcp,
            DialogTransport::Tls => gmv_pjsip::SipTransportProtocol::Tls,
        },
        association_id: 0,
        local_addr: session.local_sip_addr.parse().map_err(|_| {
            GlobalError::new_sys_error("invalid restored local SIP address", |msg| {
                error!("stream_id={}; {msg}", session.stream_id)
            })
        })?,
        remote_addr: session.remote_sip_addr.parse().map_err(|_| {
            GlobalError::new_sys_error("invalid restored remote SIP address", |msg| {
                error!("stream_id={}; {msg}", session.stream_id)
            })
        })?,
    })
}

fn stream_call_id(stream_id: &str) -> GlobalResult<String> {
    GeneralCache::stream_call_id(stream_id).ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "SIP dialog not found",
            |msg| error!("stream_id={stream_id}; {msg}"),
        )
    })
}

fn device_not_connected(device_id: &str) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::NotFound.code(),
        "device is not registered or connected",
        |msg| error!("device_id={device_id}; {msg}"),
    )
}

fn format_gb_ssrc(ssrc: u32) -> String {
    format!("{ssrc:010}")
}

fn normalize_gb_ssrc(ssrc: &str) -> GlobalResult<String> {
    let ssrc = ssrc.trim();
    if ssrc.len() != 10 || !ssrc.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "invalid GB28181 SSRC",
            |msg| error!("{msg}: ssrc={ssrc}"),
        ));
    }
    Ok(ssrc.to_string())
}

fn invite_subject(channel_id: &str, receiver_id: &str, ssrc: u32) -> String {
    let ssrc = format_gb_ssrc(ssrc);
    format!("{channel_id}:{ssrc},{receiver_id}:0")
}

pub async fn query_catalog(device_id: &str, sn: u32) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    send_native_message_and_wait(CreateDeviceMessageRequest::catalog_query(
        device_id.to_string(),
        host,
        port,
        pjsip_protocol_from_base(proto),
        sn,
    ))
    .await
}

pub async fn query_device_info(device_id: &str, sn: u32) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    send_native_message_and_wait(CreateDeviceMessageRequest::device_info_query(
        device_id.to_string(),
        host,
        port,
        pjsip_protocol_from_base(proto),
        sn,
    ))
    .await
}

pub async fn query_record_info(
    device_id: &str,
    sn: u32,
    start_time: &str,
    end_time: &str,
) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    send_native_message_and_wait(CreateDeviceMessageRequest::record_info_query(
        device_id.to_string(),
        host,
        port,
        pjsip_protocol_from_base(proto),
        sn,
        start_time,
        end_time,
    ))
    .await
}

pub async fn query_preset(device_id: &str) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    send_native_message_and_wait(CreateDeviceMessageRequest::preset_query(
        device_id.to_string(),
        host,
        port,
        pjsip_protocol_from_base(proto),
    ))
    .await
}

pub async fn send_xml_message(device_id: &str, body: String) -> GlobalResult<()> {
    let (host, port, proto) = connected_target(device_id)?;
    send_native_message_and_wait(CreateDeviceMessageRequest::xml(
        device_id.to_string(),
        host,
        port,
        pjsip_protocol_from_base(proto),
        body,
    ))
    .await
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
    send_native_message_and_wait(CreateDeviceMessageRequest::snapshot_control(
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
    .await
}

pub async fn invite_play(req: InvitePlayRequest) -> GlobalResult<()> {
    let device_id = req.device_id.clone();
    let Some(session) = Register::get_connected_device_session(&device_id) else {
        return Err(device_not_connected(&device_id));
    };
    let runtime = NativeSipRuntimeHandle::global()?;
    let protocol = pjsip_protocol_from_base(session.association.protocol);
    let operation_id = runtime.next_operation_id();
    let conf = SessionConf::get_session_by_conf();
    let sdp = req.sdp.unwrap_or_else(|| {
        build_play_sdp(gmv_pjsip::gb28181::sdp::PlaySdpOptions {
            ip: req.media_ip,
            port: req.media_port,
            ssrc: req.ssrc,
            payload_type: req.payload_type,
        })
    });
    runtime.send_invite(
        &session.association,
        SipOutboundInvite {
            operation_id,
            association_id: 0,
            protocol,
            identity: SipInviteIdentity::generate(),
            target_uri: target_uri(&req.device_id, &req.device_host, req.device_port, protocol),
            from_uri: format!("<sip:{}@{}:{}>", conf.domain_id, conf.wan_ip, conf.wan_port),
            contact_uri: format!(
                "<{}>",
                target_uri(
                    &conf.domain_id,
                    &conf.wan_ip.to_string(),
                    conf.wan_port,
                    protocol,
                )
            ),
            subject: Some(req.subject.unwrap_or_else(|| {
                invite_subject(&req.channel_id, conf.media_receiver_id(), req.ssrc)
            })),
            sdp,
        },
    )
}

pub async fn invite_play_and_wait(req: InvitePlayRequest) -> GlobalResult<GbInviteAcceptedEvent> {
    let device_id = req.device_id.clone();
    let stream_id = req.stream_id.clone();
    let Some(session) = Register::get_connected_device_session(&device_id) else {
        return Err(device_not_connected(&device_id));
    };
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let protocol = pjsip_protocol_from_base(session.association.protocol);
    let conf = SessionConf::get_session_by_conf();
    let sdp = req.sdp.clone().unwrap_or_else(|| {
        build_play_sdp(gmv_pjsip::gb28181::sdp::PlaySdpOptions {
            ip: req.media_ip,
            port: req.media_port,
            ssrc: req.ssrc,
            payload_type: req.payload_type,
        })
    });
    let identity = SipInviteIdentity::generate();
    let invite = SipOutboundInvite {
        operation_id,
        association_id: 0,
        protocol,
        identity: identity.clone(),
        target_uri: target_uri(&req.device_id, &req.device_host, req.device_port, protocol),
        from_uri: format!("<sip:{}@{}:{}>", conf.domain_id, conf.wan_ip, conf.wan_port),
        contact_uri: format!(
            "<{}>",
            target_uri(
                &conf.domain_id,
                &conf.wan_ip.to_string(),
                conf.wan_port,
                protocol,
            )
        ),
        subject: Some(req.subject.clone().unwrap_or_else(|| {
            invite_subject(&req.channel_id, conf.media_receiver_id(), req.ssrc)
        })),
        sdp,
    };
    let signal_node_id = conf.domain_id.clone();
    let now = Local::now().naive_local();
    SipDialogSessionRepository::insert_inviting(&SipDialogSession {
        stream_id: stream_id.clone(),
        device_id: device_id.clone(),
        channel_id: req.channel_id.clone(),
        session_type: req.session_type,
        signal_node_id: signal_node_id.clone(),
        media_node_id: req.media_node_id.clone(),
        ssrc: Some(format_gb_ssrc(req.ssrc)),
        call_id: identity.call_id.clone(),
        local_uri: invite.from_uri.clone(),
        remote_uri: invite.target_uri.clone(),
        local_tag: identity.local_tag.clone(),
        remote_tag: None,
        local_cseq: i64::from(identity.local_cseq),
        remote_cseq: None,
        contact_uri: None,
        route_set: Vec::new(),
        local_sip_addr: session.association.local_addr.to_string(),
        remote_sip_addr: session.association.remote_addr.to_string(),
        transport: dialog_transport(protocol),
        state: DialogState::Inviting,
        established_at: None,
        last_seen_at: now,
        expire_at: now + TimeDelta::hours(DIALOG_EXPIRE_HOURS),
        version: 0,
        created_at: now,
        updated_at: now,
    })
    .await?;
    let rx = SipRuntimeCache::global().insert_native_invite_waiter(
        operation_id,
        NativeInviteMetadata {
            device_id: device_id.clone(),
            channel_id: req.channel_id.clone(),
            stream_id: stream_id.clone(),
            ssrc: Some(req.ssrc),
        },
        INVITE_WAIT_TIMEOUT,
    );
    if let Err(err) = runtime.send_invite(&session.association, invite) {
        SipRuntimeCache::global().remove_native_invite_waiter(operation_id);
        mark_inviting_terminal(&stream_id, &signal_node_id, DialogState::Terminated).await;
        return Err(err);
    }
    match recv_with_timeout(rx, INVITE_WAIT_TIMEOUT).await {
        Ok(Ok(event)) => {
            let snapshot = &event.dialog_snapshot;
            if snapshot.call_id != identity.call_id
                || snapshot.local_tag != identity.local_tag
                || snapshot.local_cseq != identity.local_cseq
                || snapshot.protocol != protocol
            {
                rollback_established_invite(
                    &device_id,
                    &stream_id,
                    &signal_node_id,
                    &event.call_id,
                    DialogState::Orphan,
                )
                .await;
                return Err(GlobalError::new_sys_error(
                    "INVITE dialog snapshot does not match persisted identity",
                    |msg| error!("stream_id={stream_id}; call_id={}; {msg}", event.call_id),
                ));
            }
            let established_at = Local::now().naive_local();
            let fields = EstablishedDialogFields {
                remote_tag: snapshot.remote_tag.clone(),
                local_cseq: i64::from(snapshot.local_cseq),
                remote_cseq: None,
                contact_uri: Some(snapshot.remote_target.clone()),
                route_set: snapshot.route_set.clone(),
                local_sip_addr: snapshot.local_addr.to_string(),
                remote_sip_addr: snapshot.remote_addr.to_string(),
                established_at,
                last_seen_at: established_at,
                expire_at: established_at + TimeDelta::hours(DIALOG_EXPIRE_HOURS),
                updated_at: established_at,
            };
            match SipDialogSessionRepository::cas_mark_established(
                &stream_id,
                &signal_node_id,
                0,
                &fields,
            )
            .await
            {
                Ok(true) => Ok(event),
                Ok(false) => {
                    rollback_established_invite(
                        &device_id,
                        &stream_id,
                        &signal_node_id,
                        &event.call_id,
                        DialogState::Orphan,
                    )
                    .await;
                    Err(GlobalError::new_sys_error(
                        "INVITE dialog ESTABLISHED CAS lost",
                        |msg| error!("stream_id={stream_id}; call_id={}; {msg}", event.call_id),
                    ))
                }
                Err(err) => {
                    rollback_established_invite(
                        &device_id,
                        &stream_id,
                        &signal_node_id,
                        &event.call_id,
                        DialogState::Orphan,
                    )
                    .await;
                    Err(err)
                }
            }
        }
        Ok(Err(failure)) => {
            SipRuntimeCache::global().remove_stream_indexes(&stream_id, Some(&failure.call_id));
            if failure.dialog_established {
                rollback_established_invite(
                    &device_id,
                    &stream_id,
                    &signal_node_id,
                    &failure.call_id,
                    DialogState::Orphan,
                )
                .await;
            } else {
                mark_inviting_terminal(&stream_id, &signal_node_id, DialogState::Terminated).await;
            }
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
            SipRuntimeCache::global().remove_native_invite_waiter(operation_id);
            mark_inviting_terminal(&stream_id, &signal_node_id, DialogState::Orphan).await;
            Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device INVITE response timeout",
                |msg| error!("stream_id={stream_id}; {msg}; reason={reason}"),
            ))
        }
    }
}

fn dialog_transport(protocol: gmv_pjsip::SipTransportProtocol) -> DialogTransport {
    match protocol {
        gmv_pjsip::SipTransportProtocol::Udp => DialogTransport::Udp,
        gmv_pjsip::SipTransportProtocol::Tcp => DialogTransport::Tcp,
        gmv_pjsip::SipTransportProtocol::Tls => DialogTransport::Tls,
    }
}

async fn mark_inviting_terminal(stream_id: &str, signal_node_id: &str, next_state: DialogState) {
    let updated_at = Local::now().naive_local();
    match SipDialogSessionRepository::cas_transition(
        stream_id,
        signal_node_id,
        0,
        DialogState::Inviting,
        next_state,
        updated_at,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            error!("INVITING terminal CAS lost: stream_id={stream_id}; next_state={next_state}")
        }
        Err(err) => error!(
            "INVITING terminal persistence failed: stream_id={stream_id}; \
             next_state={next_state}; err={err}"
        ),
    }
}

async fn rollback_established_invite(
    device_id: &str,
    stream_id: &str,
    signal_node_id: &str,
    call_id: &str,
    next_state: DialogState,
) {
    let bye_result = send_native_dialog_and_wait(
        device_id,
        SipDialogMethod::Bye,
        call_id.to_string(),
        None,
        Vec::new(),
        BYE_WAIT_TIMEOUT,
    )
    .await;
    if let Err(err) = &bye_result {
        error!(
            "rollback established INVITE with BYE failed: stream_id={stream_id}; \
             call_id={call_id}; err={err}"
        );
    }
    SipRuntimeCache::global().remove_stream_indexes(stream_id, Some(call_id));
    mark_inviting_terminal(
        stream_id,
        signal_node_id,
        if bye_result.is_ok() {
            DialogState::Terminated
        } else {
            next_state
        },
    )
    .await;
}

pub async fn talk_invite_and_wait(req: InviteTalkRequest) -> GlobalResult<GbInviteAcceptedEvent> {
    let device_id = req.device_id.clone();
    let talk_id = req.talk_id.clone();
    let Some(session) = Register::get_connected_device_session(&device_id) else {
        return Err(device_not_connected(&device_id));
    };
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global().insert_native_invite_waiter(
        operation_id,
        NativeInviteMetadata {
            device_id: device_id.clone(),
            channel_id: req.channel_id.clone(),
            stream_id: talk_id.clone(),
            ssrc: Some(req.ssrc),
        },
        INVITE_WAIT_TIMEOUT,
    );
    let protocol = pjsip_protocol_from_base(session.association.protocol);
    let conf = SessionConf::get_session_by_conf();
    let invite = SipOutboundInvite {
        operation_id,
        association_id: 0,
        protocol,
        identity: SipInviteIdentity::generate(),
        target_uri: target_uri(&req.device_id, &req.device_host, req.device_port, protocol),
        from_uri: format!("<sip:{}@{}:{}>", conf.domain_id, conf.wan_ip, conf.wan_port),
        contact_uri: format!(
            "<{}>",
            target_uri(
                &conf.domain_id,
                &conf.wan_ip.to_string(),
                conf.wan_port,
                protocol,
            )
        ),
        subject: Some(req.subject.unwrap_or_else(|| {
            invite_subject(&req.channel_id, conf.media_receiver_id(), req.ssrc)
        })),
        sdp: build_talk_sdp(TalkSdpOptions {
            ip: req.media_ip,
            port: req.media_port,
            ssrc: req.ssrc,
            payload_type: req.payload_type,
            codec: req.codec,
            mode: req.mode,
        }),
    };
    if let Err(err) = runtime.send_invite(&session.association, invite) {
        SipRuntimeCache::global().remove_native_invite_waiter(operation_id);
        return Err(err);
    }
    match recv_with_timeout(rx, INVITE_WAIT_TIMEOUT).await {
        Ok(Ok(event)) => {
            let expected_ssrc = format_gb_ssrc(req.ssrc);
            if let Err(err) = sdp::validate_invite_answer_sdp(&event.remote_sdp, &expected_ssrc) {
                close_invite_after_answer_error(
                    &device_id,
                    &talk_id,
                    &event,
                    "invalid talk answer SDP",
                )
                .await;
                return Err(err);
            }
            Ok(event)
        }
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
            SipRuntimeCache::global().remove_native_invite_waiter(operation_id);
            Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "device talk INVITE response timeout",
                |msg| error!("talk_id={talk_id}; {msg}; reason={reason}"),
            ))
        }
    }
}

async fn close_invite_after_answer_error(
    device_id: &str,
    stream_id: &str,
    accepted: &GbInviteAcceptedEvent,
    reason: &str,
) {
    if let Err(err) = invite_stop_by_device(
        device_id,
        InviteStopRequest {
            call_id: Some(accepted.call_id.clone()),
            stream_id: Some(stream_id.to_string()),
        },
    )
    .await
    {
        error!(
            "device_id={device_id}; stream_id={stream_id}; call_id={}; \
             failed to close invalid SDP dialog: {:?}",
            accepted.call_id, err
        );
    }
    SipRuntimeCache::global().remove_stream_indexes(stream_id, Some(&accepted.call_id));
    error!(
        "device_id={device_id}; stream_id={stream_id}; call_id={}; {reason}",
        accepted.call_id
    );
}

async fn parse_media_ext_or_close(
    device_id: &str,
    stream_id: &str,
    expected_ssrc: &str,
    accepted: &GbInviteAcceptedEvent,
) -> GlobalResult<MediaExt> {
    let result = sdp::validate_invite_answer_sdp(&accepted.remote_sdp, expected_ssrc)
        .and_then(|()| sdp::parse_media_ext(accepted.remote_sdp.as_bytes()));
    match result {
        Ok(ext) => Ok(ext),
        Err(err) => {
            close_invite_after_answer_error(
                device_id,
                stream_id,
                accepted,
                "invalid play answer SDP",
            )
            .await;
            Err(err)
        }
    }
}

pub async fn play_live_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_node_id: &str,
    media_ip: &str,
    media_port: u16,
    trans_mode: TransMode,
    ssrc: &str,
    stream_id: &str,
) -> GlobalResult<(GbInviteAcceptedEvent, MediaExt)> {
    let (host, port, proto) = connected_target(device_id)?;
    let ssrc = normalize_gb_ssrc(ssrc)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::play_live(channel_id, media_ip, media_port, trans_mode, &ssrc, true);
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        media_node_id: media_node_id.to_string(),
        session_type: DialogSessionType::Live,
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
    let ext = parse_media_ext_or_close(device_id, stream_id, &ssrc, &accepted).await?;
    Ok((accepted, ext))
}

pub async fn play_back_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_node_id: &str,
    media_ip: &str,
    media_port: u16,
    trans_mode: TransMode,
    ssrc: &str,
    stream_id: &str,
    st: u32,
    et: u32,
) -> GlobalResult<(GbInviteAcceptedEvent, MediaExt)> {
    let (host, port, proto) = connected_target(device_id)?;
    let ssrc = normalize_gb_ssrc(ssrc)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::playback(
        channel_id, media_ip, media_port, trans_mode, &ssrc, st, et, true,
    );
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        media_node_id: media_node_id.to_string(),
        session_type: DialogSessionType::Playback,
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
    let ext = parse_media_ext_or_close(device_id, stream_id, &ssrc, &accepted).await?;
    Ok((accepted, ext))
}

pub async fn download_invite_wait(
    device_id: &str,
    channel_id: &str,
    media_node_id: &str,
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
    let ssrc = normalize_gb_ssrc(ssrc)?;
    let ssrc_u32 = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let protocol = transport_protocol(trans_mode, proto);
    let sdp = sdp::download(
        channel_id, media_ip, media_port, trans_mode, &ssrc, st, et, speed, true,
    );
    let accepted = invite_play_and_wait(InvitePlayRequest {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        stream_id: stream_id.to_string(),
        media_node_id: media_node_id.to_string(),
        session_type: DialogSessionType::Download,
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
    let ext = parse_media_ext_or_close(device_id, stream_id, &ssrc, &accepted).await?;
    Ok((accepted, ext))
}

pub async fn invite_stop_by_device(device_id: &str, req: InviteStopRequest) -> GlobalResult<()> {
    let stream_id = req.stream_id.clone();
    let call_id = req
        .call_id
        .or_else(|| {
            req.stream_id
                .as_deref()
                .and_then(GeneralCache::stream_call_id)
        })
        .ok_or_else(|| {
            GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "SIP dialog not found",
                |msg| error!("device_id={device_id}; {msg}"),
            )
        })?;
    let reservation = match stream_id.as_deref() {
        Some(stream_id) => reserve_durable_dialog_request(stream_id, SipDialogMethod::Bye).await?,
        None => None,
    };
    if stream_id
        .as_deref()
        .is_some_and(GeneralCache::stream_is_restored)
    {
        let session = load_durable_dialog(stream_id.as_deref().unwrap_or_default()).await?;
        send_restored_dialog_and_wait(
            device_id,
            SipDialogMethod::Bye,
            &session,
            None,
            Vec::new(),
            BYE_WAIT_TIMEOUT,
        )
        .await?;
    } else {
        send_native_dialog_and_wait(
            device_id,
            SipDialogMethod::Bye,
            call_id.clone(),
            None,
            Vec::new(),
            BYE_WAIT_TIMEOUT,
        )
        .await?;
    }
    if let (Some(stream_id), Some(reservation)) = (stream_id.as_deref(), reservation) {
        let updated_at = Local::now().naive_local();
        let persisted = SipDialogSessionRepository::cas_transition(
            stream_id,
            &reservation.signal_node_id,
            reservation.version,
            DialogState::Terminating,
            DialogState::Terminated,
            updated_at,
        )
        .await?;
        if !persisted {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "durable SIP dialog TERMINATED CAS lost",
                |msg| error!("stream_id={stream_id}; call_id={call_id}; {msg}"),
            ));
        }
    }
    Ok(())
}

pub async fn invite_stop_by_stream(stream_id: &str) -> GlobalResult<()> {
    let Some(call_id) = GeneralCache::stream_call_id(stream_id) else {
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
            call_id: Some(call_id),
            stream_id: Some(stream_id.to_string()),
        },
    )
    .await
}

pub async fn play_seek(device_id: &str, stream_id: &str, seek_second: u32) -> GlobalResult<()> {
    let call_id = stream_call_id(stream_id)?;
    reserve_durable_dialog_request(stream_id, SipDialogMethod::Info).await?;
    let content_type = Some(CONTENT_TYPE_MANSRTSP.to_string());
    let body = build_mansrtsp_seek_body(f64::from(seek_second), 1).into_bytes();
    if GeneralCache::stream_is_restored(stream_id) {
        let session = load_durable_dialog(stream_id).await?;
        send_restored_dialog_and_wait(
            device_id,
            SipDialogMethod::Info,
            &session,
            content_type,
            body,
            REQUEST_WAIT_TIMEOUT,
        )
        .await
    } else {
        send_native_dialog_and_wait(
            device_id,
            SipDialogMethod::Info,
            call_id,
            content_type,
            body,
            REQUEST_WAIT_TIMEOUT,
        )
        .await
    }
}

pub async fn play_speed(device_id: &str, stream_id: &str, speed: f32) -> GlobalResult<()> {
    let call_id = stream_call_id(stream_id)?;
    reserve_durable_dialog_request(stream_id, SipDialogMethod::Info).await?;
    let content_type = Some(CONTENT_TYPE_MANSRTSP.to_string());
    let body = build_mansrtsp_speed_body(speed, None, 1).into_bytes();
    if GeneralCache::stream_is_restored(stream_id) {
        let session = load_durable_dialog(stream_id).await?;
        send_restored_dialog_and_wait(
            device_id,
            SipDialogMethod::Info,
            &session,
            content_type,
            body,
            REQUEST_WAIT_TIMEOUT,
        )
        .await
    } else {
        send_native_dialog_and_wait(
            device_id,
            SipDialogMethod::Info,
            call_id,
            content_type,
            body,
            REQUEST_WAIT_TIMEOUT,
        )
        .await
    }
}

async fn load_durable_dialog(stream_id: &str) -> GlobalResult<SipDialogSession> {
    SipDialogSessionRepository::find_by_stream_id(stream_id)
        .await?
        .ok_or_else(|| {
            GlobalError::new_biz_error(
                BaseErrorCode::NotFound.code(),
                "durable SIP dialog not found",
                |msg| error!("stream_id={stream_id}; {msg}"),
            )
        })
}

async fn reserve_durable_dialog_request(
    stream_id: &str,
    method: SipDialogMethod,
) -> GlobalResult<Option<DurableDialogReservation>> {
    let Some(session) = SipDialogSessionRepository::find_by_stream_id(stream_id).await? else {
        return Ok(None);
    };
    let current_node_id = SessionConf::get_session_by_conf().domain_id;
    if session.signal_node_id != current_node_id {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "durable SIP dialog belongs to another signal node",
            |msg| {
                error!(
                    "stream_id={stream_id}; owner={}; current={current_node_id}; {msg}",
                    session.signal_node_id
                )
            },
        ));
    }
    let next_cseq = session.local_cseq.checked_add(1).ok_or_else(|| {
        GlobalError::new_sys_error("durable SIP dialog CSeq overflow", |msg| {
            error!("stream_id={stream_id}; {msg}")
        })
    })?;
    let updated_at = Local::now().naive_local();
    let reserved = match (method, session.state) {
        (SipDialogMethod::Info, DialogState::Established)
        | (SipDialogMethod::Bye, DialogState::Terminating) => {
            SipDialogSessionRepository::cas_reserve_local_cseq(
                stream_id,
                &session.signal_node_id,
                session.version,
                session.local_cseq,
                next_cseq,
                updated_at,
            )
            .await?
        }
        (SipDialogMethod::Bye, DialogState::Established) => {
            SipDialogSessionRepository::cas_begin_terminating(
                stream_id,
                &session.signal_node_id,
                session.version,
                session.local_cseq,
                next_cseq,
                updated_at,
            )
            .await?
        }
        _ => {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "durable SIP dialog does not allow this request",
                |msg| {
                    error!(
                        "stream_id={stream_id}; method={method:?}; state={}; {msg}",
                        session.state
                    )
                },
            ));
        }
    };
    if !reserved {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "durable SIP dialog CSeq reservation lost",
            |msg| error!("stream_id={stream_id}; method={method:?}; {msg}"),
        ));
    }
    Ok(Some(DurableDialogReservation {
        signal_node_id: session.signal_node_id,
        version: session.version + 1,
    }))
}

#[cfg(test)]
mod tests {
    use super::{build_ptz_command, invite_subject, normalize_gb_ssrc};
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

    #[test]
    fn invite_subject_keeps_gb28181_receiver_leg_zero() {
        assert_eq!(
            invite_subject("34020000001320000102", "34020000002000000001", 4423),
            "34020000001320000102:0000004423,34020000002000000001:0"
        );
    }

    #[test]
    fn gb28181_ssrc_must_be_ten_digits() {
        assert!(normalize_gb_ssrc("0000004423").is_ok());
        assert!(normalize_gb_ssrc("4423").is_err());
    }
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
