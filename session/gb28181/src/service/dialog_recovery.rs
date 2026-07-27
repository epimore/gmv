use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::dashmap::DashSet;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Protocol};
use base::once_cell::sync::Lazy;
use gmv_domain::info::obj::StreamKey;

use crate::gb::SessionConf;
use crate::gb::sip::runtime_cache::SipRuntimeCache;
use crate::register::core::{DeviceSession, Register};
use crate::service::stream_rpc;
use crate::state::session::{AccessMode, Cache};
use crate::storage::dialog_session::{
    DialogSessionType, DialogState, DialogTransport, SipDialogSession, SipDialogSessionRepository,
};
use crate::storage::entity::{GmvDevice, GmvOauth};
use crate::storage::recording;

const RECOVERY_PAGE_SIZE: u32 = 200;

static RUNTIME_DIALOG_CONFLICTS: Lazy<DashSet<String>> = Lazy::new(DashSet::new);

pub fn runtime_dialog_conflict_count() -> usize {
    RUNTIME_DIALOG_CONFLICTS.len()
}

pub async fn run_startup_recovery() -> GlobalResult<()> {
    recover_owned_dialogs().await
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
    if session.state == DialogState::Inviting {
        if session.created_at + TimeDelta::seconds(60) > now {
            return Ok(());
        }
        if !cleanup_setup_media(session).await? {
            return Ok(());
        }
        mark_orphan(session).await?;
        return Ok(());
    }
    if session.transport == DialogTransport::Tls
        || (session.state == DialogState::Terminating && session.expire_at <= now)
    {
        mark_orphan(session).await?;
        return Ok(());
    }
    if let Err(err) = validate_registration_epoch(session).await {
        mark_orphan(session).await?;
        return Err(err);
    }

    let ssrc = session
        .ssrc
        .as_deref()
        .ok_or_else(|| invalid_recovery(session, "durable dialog SSRC is missing"))?
        .parse::<u32>()
        .map_err(|_| invalid_recovery(session, "durable dialog SSRC is invalid"))?;
    if session.session_type == DialogSessionType::Talk {
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
        if session.state == DialogState::Established {
            touch_active_dialog(session).await?;
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
            close_terminal_reason: None,
            guard_lease: None,
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
    let media_online = query_media_online(session, ssrc).await?;

    if !media_online && session.session_type == DialogSessionType::Download {
        recording::mark_stream_restart_interrupted(&session.stream_id).await?;
    }

    if session.transport == DialogTransport::Udp {
        if let Err(err) = ensure_udp_device_session(session).await {
            mark_orphan(session).await?;
            return Err(err);
        }
    }

    if media_online {
        if session.state == DialogState::Established {
            touch_active_dialog(session).await?;
        }
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
        } else if session.session_type == DialogSessionType::Playback {
            crate::service::hook_serv::restore_playback_pause_deadline(&session.stream_id).await;
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

pub async fn run_reconciliation(cancel: base::tokio_util::sync::CancellationToken) {
    let signal_node_id = SessionConf::get_session_by_conf().domain_id;
    let states = [
        DialogState::Inviting,
        DialogState::Established,
        DialogState::Terminating,
    ];
    let mut cursor: Option<String> = None;
    loop {
        base::tokio::select! {
            _ = cancel.cancelled() => break,
            _ = base::tokio::time::sleep(Duration::from_secs(60)) => {}
        }
        reconcile_runtime_dialog_conflicts(&signal_node_id).await;
        let page = match SipDialogSessionRepository::page_owned_by_states(
            &signal_node_id,
            &states,
            cursor.as_deref(),
            RECOVERY_PAGE_SIZE,
        )
        .await
        {
            Ok(page) => page,
            Err(err) => {
                error!("dialog reconciliation scan failed: {err}");
                continue;
            }
        };
        if page.is_empty() {
            cursor = None;
            continue;
        }
        for dialog in &page {
            match dialog.state {
                DialogState::Inviting
                    if dialog.created_at + TimeDelta::seconds(60) <= Local::now().naive_local() =>
                {
                    match cleanup_setup_media(dialog).await {
                        Ok(true) => {
                            if let Err(err) = mark_orphan(dialog).await {
                                warn!(
                                    "dialog setup reconciliation failed: stream_id={}, err={err}",
                                    dialog.stream_id
                                );
                            }
                        }
                        Ok(false) => {}
                        Err(err) => {
                            warn!(
                                "dialog setup media cleanup will retry: stream_id={}, err={err}",
                                dialog.stream_id
                            );
                        }
                    }
                }
                DialogState::Terminating => {
                    if dialog.session_type == DialogSessionType::Talk {
                        crate::service::talk_close::begin(dialog.stream_id.clone());
                    } else {
                        crate::service::stream_close::begin(dialog.stream_id.clone());
                    }
                }
                DialogState::Established => {
                    let probe = if dialog.session_type == DialogSessionType::Talk {
                        base::tokio::time::timeout(
                            Duration::from_secs(3),
                            query_talk_online(dialog),
                        )
                        .await
                    } else if let Some(ssrc) = dialog
                        .ssrc
                        .as_deref()
                        .and_then(|value| value.parse::<u32>().ok())
                    {
                        base::tokio::time::timeout(
                            Duration::from_secs(3),
                            query_media_online(dialog, ssrc),
                        )
                        .await
                    } else {
                        continue;
                    };
                    match probe {
                        Ok(Ok(true)) => {
                            if let Err(err) = touch_active_dialog(dialog).await {
                                warn!(
                                    "dialog activity refresh failed: stream_id={}, err={err}",
                                    dialog.stream_id
                                );
                            }
                        }
                        Ok(Ok(false)) => {
                            if let Err(err) = recover_dialog(dialog).await {
                                warn!(
                                    "dialog media reconciliation failed: stream_id={}, err={err}",
                                    dialog.stream_id
                                );
                            }
                        }
                        Ok(Err(err)) => warn!(
                            "dialog media probe failed and will retry: stream_id={}, err={err}",
                            dialog.stream_id
                        ),
                        Err(_) => warn!(
                            "dialog media probe timed out and will retry: stream_id={}",
                            dialog.stream_id
                        ),
                    }
                }
                _ => {}
            }
        }
        cursor = if page.len() < RECOVERY_PAGE_SIZE as usize {
            None
        } else {
            page.last().map(|dialog| dialog.stream_id.clone())
        };
    }
}

async fn reconcile_runtime_dialog_conflicts(signal_node_id: &str) {
    let runtime_ids = Cache::stream_ids()
        .into_iter()
        .chain(Cache::talk_ids())
        .collect::<HashSet<_>>();
    RUNTIME_DIALOG_CONFLICTS.retain(|stream_id| runtime_ids.contains(stream_id));
    for stream_id in runtime_ids {
        let conflict = match SipDialogSessionRepository::find_by_stream_id(&stream_id).await {
            Ok(Some(dialog))
                if dialog.signal_node_id == signal_node_id
                    && matches!(
                        dialog.state,
                        DialogState::Inviting | DialogState::Established | DialogState::Terminating
                    ) =>
            {
                None
            }
            Ok(Some(dialog)) if dialog.signal_node_id != signal_node_id => Some("wrong_owner"),
            Ok(Some(_)) => Some("dialog_terminal"),
            Ok(None) => Some("dialog_missing"),
            Err(err) => {
                warn!(
                    "runtime dialog reverse reconciliation lookup failed: stream_id={stream_id}, err={err}"
                );
                continue;
            }
        };
        match conflict {
            Some(reason) => {
                if RUNTIME_DIALOG_CONFLICTS.insert(stream_id.clone()) {
                    warn!(
                        "runtime dialog state changed: state=conflict, outcome=requires_attention, stream_id={stream_id}, reason={reason}; automatic media cleanup skipped because reverse ownership fencing is incomplete"
                    );
                }
            }
            None => {
                if RUNTIME_DIALOG_CONFLICTS.remove(&stream_id).is_some() {
                    info!(
                        "runtime dialog state changed: state=consistent, previous_state=conflict, outcome=recovered, stream_id={stream_id}"
                    );
                }
            }
        }
    }
}

async fn cleanup_setup_media(dialog: &SipDialogSession) -> GlobalResult<bool> {
    let Some(node) = crate::state::StreamNodeRegistry::get(&dialog.media_node_id) else {
        return Ok(false);
    };
    if dialog.session_type == DialogSessionType::Talk {
        stream_rpc::talk_close(&node, &dialog.stream_id).await
    } else {
        stream_rpc::stop_receive(&node, &dialog.stream_id, "setup_deadline").await
    }?;
    Ok(true)
}

async fn touch_active_dialog(dialog: &SipDialogSession) -> GlobalResult<()> {
    let now = Local::now().naive_local();
    let _ = SipDialogSessionRepository::cas_touch(
        &dialog.stream_id,
        &dialog.signal_node_id,
        dialog.version,
        now,
        now + TimeDelta::hours(8),
    )
    .await?;
    Ok(())
}

pub async fn run_history_retention(cancel: base::tokio_util::sync::CancellationToken) {
    let conf = SessionConf::get_session_by_conf();
    if conf.dialog_history_retention_days == 0 {
        return;
    }
    loop {
        let cutoff = Local::now().naive_local()
            - TimeDelta::days(i64::from(conf.dialog_history_retention_days));
        let mut deleted = 0_u64;
        let mut batches = 0_u64;
        loop {
            match SipDialogSessionRepository::delete_terminal_before(&conf.domain_id, cutoff, 500)
                .await
            {
                Ok(0) => break,
                Ok(count) => {
                    deleted += count;
                    batches += 1;
                }
                Err(err) => {
                    error!(
                        "dialog history retention failed: retention_days={}, deleted={}, batches={}, err={err}",
                        conf.dialog_history_retention_days, deleted, batches
                    );
                    break;
                }
            }
        }
        info!(
            "dialog history retention completed: retention_days={}, cutoff={}, deleted={}, batches={}",
            conf.dialog_history_retention_days, cutoff, deleted, batches
        );
        base::tokio::select! {
            _ = cancel.cancelled() => break,
            _ = base::tokio::time::sleep(Duration::from_secs(86_400)) => {}
        }
    }
}

async fn query_talk_online(session: &SipDialogSession) -> GlobalResult<bool> {
    let node = crate::guard_integration::ensure_stream_node(&session.media_node_id).await?;
    stream_rpc::talk_online(&node, &session.stream_id).await
}

async fn query_media_online(session: &SipDialogSession, ssrc: u32) -> GlobalResult<bool> {
    let node = crate::guard_integration::ensure_stream_node(&session.media_node_id).await?;
    stream_rpc::stream_online(
        &node,
        &StreamKey {
            ssrc,
            stream_id: Some(session.stream_id.clone()),
        },
    )
    .await
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
        oauth.heartbeat_sec_u8()?,
        Duration::from_secs(remaining),
    );
    device_session.set_optional_registration_epoch_id(device.registration_epoch_id);
    device_session.mark_registration_snapshot_restored();
    device_session.set_registration_identity(
        device.registration_call_id.unwrap_or_default(),
        device
            .registration_cseq
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default(),
    );
    device_session.set_gb_version(device.gb_version);
    device_session.mark_registration_ready();
    if device.enable_lr != 0 {
        device_session.enable_lr();
    }
    Register::register_device(Arc::from(session.device_id.as_str()), device_session).map(|_| ())
}

async fn validate_registration_epoch(session: &SipDialogSession) -> GlobalResult<()> {
    let device = GmvDevice::query_gmv_device_by_device_id(&session.device_id)
        .await?
        .ok_or_else(|| invalid_recovery(session, "device registration snapshot is missing"))?;
    if device.registration_epoch_closed_at.is_some() {
        return Err(invalid_recovery(
            session,
            "device registration epoch is closed",
        ));
    }
    if device.registration_epoch_id != session.registration_epoch_id {
        return Err(invalid_recovery(
            session,
            "durable dialog registration epoch mismatch",
        ));
    }
    Ok(())
}

async fn mark_orphan(session: &SipDialogSession) -> GlobalResult<()> {
    let changed = SipDialogSessionRepository::cas_mark_terminal(
        &session.stream_id,
        &session.signal_node_id,
        session.version,
        session.state,
        DialogState::Orphan,
        "recovery_failed",
        Some("RECOVERY_FAILED"),
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
