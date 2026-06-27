use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Protocol};
use gmv_domain::info::obj::{StreamKey, TalkCloseReq};

use crate::gb::SessionConf;
use crate::gb::sip::runtime_cache::SipRuntimeCache;
use crate::http::client::{HttpClient, HttpStream};
use crate::register::core::{DeviceSession, Register};
use crate::state::StreamConf;
use crate::state::session::{AccessMode, Cache};
use crate::storage::dialog_session::{
    DialogSessionType, DialogState, DialogTransport, SipDialogSession, SipDialogSessionRepository,
};
use crate::storage::entity::{GmvDevice, GmvOauth};

const RECOVERY_PAGE_SIZE: u32 = 200;

pub async fn run_startup_recovery() {
    if let Err(err) = recover_owned_dialogs().await {
        error!("startup durable dialog recovery failed: {err}");
    }
}

pub(crate) async fn recover_owned_dialogs() -> GlobalResult<()> {
    let signal_node_id = SessionConf::get_session_by_conf().domain_id;
    let states = [
        DialogState::Inviting,
        DialogState::Established,
        DialogState::Terminating,
    ];
    let mut cursor = None;
    loop {
        let page = SipDialogSessionRepository::page_owned_by_states(
            &signal_node_id,
            &states,
            cursor.as_deref(),
            RECOVERY_PAGE_SIZE,
        )
        .await?;
        if page.is_empty() {
            break;
        }
        for session in &page {
            if let Err(err) = recover_dialog(session).await {
                warn!(
                    "recover durable dialog failed: stream_id={}, call_id={}, err={err}",
                    session.stream_id, session.call_id
                );
            }
        }
        cursor = page.last().map(|session| session.stream_id.clone());
        if page.len() < RECOVERY_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(())
}

pub(crate) async fn recover_dialog(session: &SipDialogSession) -> GlobalResult<()> {
    let now = Local::now().naive_local();
    if session.expire_at <= now
        || session.state == DialogState::Inviting
        || session.transport == DialogTransport::Tls
    {
        mark_orphan(session).await?;
        return Ok(());
    }

    let ssrc = session
        .ssrc
        .as_deref()
        .ok_or_else(|| invalid_recovery(session, "durable dialog SSRC is missing"))?
        .parse::<u32>()
        .map_err(|_| invalid_recovery(session, "durable dialog SSRC is invalid"))?;
    if session.session_type == DialogSessionType::Talk {
        if StreamConf::get_stream_conf()
            .node_map
            .get(&session.media_node_id)
            .is_none()
        {
            mark_orphan(session).await?;
            return Ok(());
        }
        if session.transport == DialogTransport::Udp
            && let Err(err) = ensure_udp_device_session(session).await
        {
            mark_orphan(session).await?;
            return Err(err);
        }
        if !query_talk_online(session).await? {
            mark_orphan(session).await?;
            return Ok(());
        }
        if !Cache::talk_map_insert(crate::state::session::TalkSessionState {
            talk_id: session.stream_id.clone(),
            device_id: session.device_id.clone(),
            channel_id: session.channel_id.clone(),
            ssrc,
            stream_node_name: session.media_node_id.clone(),
            call_id: session.call_id.clone(),
            seq: u32::try_from(session.local_cseq).unwrap_or(u32::MAX),
            restored: true,
            closing_generation: None,
            bye_inflight_seq: None,
            close_last_error: None,
        }) {
            mark_orphan(session).await?;
            return Ok(());
        }
        SipRuntimeCache::global()
            .restore_stream_index(session.call_id.clone(), session.stream_id.clone());
        info!(
            "restored durable talk: talk_id={}, device_id={}, media_node={}, transport={}",
            session.stream_id, session.device_id, session.media_node_id, session.transport
        );
        if session.state == DialogState::Terminating {
            crate::service::talk_close::begin(session.stream_id.clone());
        }
        return Ok(());
    }
    let access_mode = access_mode(session.session_type)?;
    if StreamConf::get_stream_conf()
        .node_map
        .get(&session.media_node_id)
        .is_none()
    {
        mark_orphan(session).await?;
        return Ok(());
    }
    let media_online = query_media_online(session, ssrc).await?;

    if session.transport == DialogTransport::Udp {
        if let Err(err) = ensure_udp_device_session(session).await {
            mark_orphan(session).await?;
            return Err(err);
        }
    }

    if media_online {
        if !Cache::stream_map_insert_restored(
            session.stream_id.clone(),
            session.device_id.clone(),
            session.channel_id.clone(),
            ssrc,
            session.media_node_id.clone(),
            session.call_id.clone(),
            u32::try_from(session.local_cseq).unwrap_or(u32::MAX),
            access_mode,
        ) {
            mark_orphan(session).await?;
            return Ok(());
        }
        Cache::device_map_insert_restored(
            session.device_id.clone(),
            session.channel_id.clone(),
            session.ssrc.clone().unwrap_or_default(),
            session.stream_id.clone(),
            access_mode,
        );
        SipRuntimeCache::global()
            .restore_stream_index(session.call_id.clone(), session.stream_id.clone());
        info!(
            "restored durable stream: stream_id={}, device_id={}, media_node={}, transport={}",
            session.stream_id, session.device_id, session.media_node_id, session.transport
        );
        if session.state == DialogState::Terminating {
            crate::service::stream_close::begin(session.stream_id.clone());
        }
        return Ok(());
    }

    if session.state == DialogState::Established {
        let _ = SipDialogSessionRepository::cas_transition(
            &session.stream_id,
            &session.signal_node_id,
            session.version,
            DialogState::Established,
            DialogState::Terminating,
            now,
        )
        .await?;
    }
    if Cache::stream_map_insert_restored(
        session.stream_id.clone(),
        session.device_id.clone(),
        session.channel_id.clone(),
        ssrc,
        session.media_node_id.clone(),
        session.call_id.clone(),
        u32::try_from(session.local_cseq).unwrap_or(u32::MAX),
        access_mode,
    ) {
        SipRuntimeCache::global()
            .restore_stream_index(session.call_id.clone(), session.stream_id.clone());
        crate::service::stream_close::begin(session.stream_id.clone());
    }
    Ok(())
}

async fn query_talk_online(session: &SipDialogSession) -> GlobalResult<bool> {
    let stream_conf = StreamConf::get_stream_conf();
    let node = stream_conf
        .node_map
        .get(&session.media_node_id)
        .ok_or_else(|| invalid_recovery(session, "configured media node is missing"))?;
    let client = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
    let response = client
        .talk_online(&TalkCloseReq {
            talk_id: session.stream_id.clone(),
        })
        .await
        .hand_log(|message| error!("{message}"))?;
    Ok(response.code == 200 && response.data == Some(true))
}

async fn query_media_online(session: &SipDialogSession, ssrc: u32) -> GlobalResult<bool> {
    let stream_conf = StreamConf::get_stream_conf();
    let node = stream_conf
        .node_map
        .get(&session.media_node_id)
        .ok_or_else(|| invalid_recovery(session, "configured media node is missing"))?;
    let client = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
    let response = client
        .stream_online(&StreamKey {
            ssrc,
            stream_id: Some(session.stream_id.clone()),
        })
        .await
        .hand_log(|message| error!("{message}"))?;
    Ok(response.code == 200 && response.data == Some(true))
}

async fn ensure_udp_device_session(session: &SipDialogSession) -> GlobalResult<()> {
    if Register::has_session(&session.device_id) {
        return Ok(());
    }
    let oauth = GmvOauth::read_gmv_oauth_by_device_id(&session.device_id)
        .await?
        .ok_or_else(|| invalid_recovery(session, "enabled device authorization is missing"))?;
    let device = GmvDevice::query_gmv_device_by_device_id(&session.device_id)
        .await?
        .ok_or_else(|| invalid_recovery(session, "device registration snapshot is missing"))?;
    if !device.transport.eq_ignore_ascii_case("UDP") {
        return Err(invalid_recovery(
            session,
            "device registration transport does not match durable dialog",
        ));
    }
    let now = Local::now().naive_local();
    let registration_expires_at =
        device.register_time + TimeDelta::seconds(i64::from(device.register_expires));
    let online_expires_at = device
        .online_expire_time
        .ok_or_else(|| invalid_recovery(session, "device online expiry is missing"))?;
    if registration_expires_at <= now || online_expires_at <= now {
        return Err(invalid_recovery(
            session,
            "device registration or online lease has expired",
        ));
    }
    let stored_device_addr = device
        .local_addr
        .parse::<SocketAddr>()
        .map_err(|_| invalid_recovery(session, "stored device address is invalid"))?;
    let remote_addr = session
        .remote_sip_addr
        .parse::<SocketAddr>()
        .map_err(|_| invalid_recovery(session, "durable remote SIP address is invalid"))?;
    if stored_device_addr.ip() != remote_addr.ip() {
        return Err(invalid_recovery(
            session,
            "stored device IP does not match durable dialog",
        ));
    }
    let conf = SessionConf::get_session_by_conf();
    let association = Association::new(
        SocketAddr::new(conf.wan_ip.into(), conf.wan_port),
        remote_addr,
        Protocol::UDP,
    );
    let remaining = registration_expires_at
        .signed_duration_since(now)
        .num_seconds()
        .max(1) as u64;
    let mut device_session = DeviceSession::build(
        device.contact_uri,
        association,
        oauth.heartbeat_sec,
        Duration::from_secs(remaining),
    );
    device_session.set_gb_version(device.gb_version);
    if device.enable_lr != 0 {
        device_session.enable_lr();
    }
    Register::register_device(Arc::from(session.device_id.as_str()), device_session)
}

async fn mark_orphan(session: &SipDialogSession) -> GlobalResult<()> {
    let changed = SipDialogSessionRepository::cas_transition(
        &session.stream_id,
        &session.signal_node_id,
        session.version,
        session.state,
        DialogState::Orphan,
        Local::now().naive_local(),
    )
    .await?;
    if !changed {
        warn!(
            "mark recovered dialog ORPHAN CAS lost: stream_id={}",
            session.stream_id
        );
    }
    Ok(())
}

fn access_mode(session_type: DialogSessionType) -> GlobalResult<AccessMode> {
    match session_type {
        DialogSessionType::Live => Ok(AccessMode::Live),
        DialogSessionType::Playback => Ok(AccessMode::Back),
        DialogSessionType::Download => Ok(AccessMode::Down),
        DialogSessionType::Talk => Err(GlobalError::new_sys_error(
            "TALK durable recovery is not supported",
            |message| error!("{message}"),
        )),
    }
}

fn invalid_recovery(session: &SipDialogSession, message: &str) -> GlobalError {
    GlobalError::new_sys_error(message, |log_message| {
        error!(
            "stream_id={}; device_id={}; {log_message}",
            session.stream_id, session.device_id
        )
    })
}
