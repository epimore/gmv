use std::sync::Arc;

use base::chrono::Local;
use base::log::{debug, error, info, warn};
use base::tokio::time::Instant;

use crate::gb::sip::command as sip_command;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, StreamByeCommand};
use crate::storage::dialog_session::{DialogState, SipDialogSessionRepository};

pub fn begin(stream_id: String) {
    begin_with_reason(stream_id, "session_close");
}

pub fn begin_with_reason(stream_id: String, terminal_reason: &str) {
    crate::service::playback_presence::clear_for_stream(&stream_id);
    let Some(start) = Cache::stream_close_begin(&stream_id, terminal_reason) else {
        return;
    };
    if !start.newly_started {
        return;
    }

    let Some(session) = Register::get_device_session(&start.device_id) else {
        if Cache::stream_is_restored(&stream_id) {
            warn!(
                "restored stream close waiting for current device transport: \
                 stream_id={stream_id}, device_id={}",
                start.device_id
            );
            return;
        }
        force_cleanup(
            &stream_id,
            start.generation,
            "device registration unavailable",
        );
        return;
    };
    let close_timeout = session.reconnect_timeout(Instant::now());
    if close_timeout.is_zero() {
        force_cleanup(&stream_id, start.generation, "close deadline expired");
        return;
    }
    if let Err(err) = Register::scheduler().insert_register(
        TimeScheduleKey::StreamClosing(Arc::from(stream_id.as_str()), start.generation),
        close_timeout,
    ) {
        force_cleanup(
            &stream_id,
            start.generation,
            &format!("schedule close deadline failed: {err}"),
        );
        return;
    }
    retry_stream(stream_id);
}

pub fn retry_device(device_id: &str) {
    for stream_id in Cache::stream_close_ids_by_device(device_id) {
        retry_stream(stream_id);
    }
}

fn retry_stream(stream_id: String) {
    let Some(command) = Cache::stream_close_take_bye(&stream_id) else {
        return;
    };
    base::tokio::spawn(send_bye(command));
}

async fn send_bye(command: StreamByeCommand) {
    let stream_id = command.stream_id.clone();
    let generation = command.generation;
    let seq = command.seq;
    let device_id = command.device_id.clone();
    stop_media_runtime(
        &command.stream_id,
        &command.stream_node_name,
        &command.terminal_reason,
    )
    .await;
    let result = sip_command::invite_stop_by_device(
        &command.device_id,
        crate::gb::sip::InviteStopRequest {
            call_id: Some(command.call_id.clone()),
            stream_id: Some(command.stream_id.clone()),
            terminal_reason: command.terminal_reason,
        },
    )
    .await;

    match result {
        Ok(()) => {
            if let Some(info) = Cache::stream_close_complete(&stream_id, generation) {
                info!(
                    "stream close completed: stage=close_finalize, outcome=closed, device_id={}, channel_id={}, stream_id={}, ssrc={}, call_id={}, generation={}",
                    info.device_id,
                    info.channel_id,
                    info.stream_id,
                    info.ssrc,
                    info.call_id,
                    info.generation
                );
                release_guard_lease(info.guard_lease);
            }
        }
        Err(err) => mark_failed(
            &stream_id,
            generation,
            seq,
            &device_id,
            err.to_string(),
            false,
        ),
    }
}

fn mark_failed(
    stream_id: &str,
    generation: u64,
    seq: u32,
    device_id: &str,
    reason: String,
    retry_if_connected: bool,
) {
    if Cache::stream_close_mark_failed(stream_id, generation, seq, reason.clone()) {
        warn!(
            "stream BYE pending retry: stream_id={}, generation={}, cseq={}, reason={}",
            stream_id, generation, seq, reason
        );
        if retry_if_connected && Register::get_connected_device_session(device_id).is_some() {
            retry_stream(stream_id.to_string());
        }
    }
}

pub(crate) fn force_cleanup(stream_id: &str, generation: u64, reason: &str) {
    if let Some(info) = Cache::stream_close_force(stream_id, generation) {
        warn!(
            "stream close forced: stage=force_finalize, outcome=forced, device_id={}, channel_id={}, stream_id={}, ssrc={}, call_id={}, generation={}, trigger_reason={}, last_error={}",
            info.device_id,
            info.channel_id,
            info.stream_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason,
            info.last_error.as_deref().unwrap_or("none")
        );
        release_guard_lease(info.guard_lease);
        let stream_id = info.stream_id;
        let stream_node_name = info.stream_node_name;
        base::tokio::spawn(async move {
            stop_media_runtime(&stream_id, &stream_node_name, "force_cleanup").await;
            finalize_durable_dialog_as_orphan("stream", &stream_id).await;
        });
    } else {
        debug!(
            "ignore stale stream force cleanup: stream_id={}, generation={}",
            stream_id, generation
        );
    }
}

async fn stop_media_runtime(stream_id: &str, stream_node_name: &str, reason: &str) {
    if stream_node_name.is_empty() {
        return;
    }
    let node = match crate::guard_integration::ensure_stream_node(stream_node_name).await {
        Ok(node) => node,
        Err(err) => {
            warn!(
                "stream media cleanup deferred: stage=resolve_stream_node, outcome=unavailable, stream_id={}, stream_node={}, reason={}, error={}",
                stream_id, stream_node_name, reason, err
            );
            return;
        }
    };
    if let Err(err) = crate::service::stream_rpc::stop_receive(&node, stream_id, reason).await {
        warn!(
            "stream media cleanup failed: stage=stop_receive, outcome=failed, stream_id={}, stream_node={}, reason={}, error={}",
            stream_id, stream_node_name, reason, err
        );
    }
}

pub(crate) async fn finalize_durable_dialog_as_orphan(resource_kind: &str, resource_id: &str) {
    finalize_durable_dialog_as_orphan_inner(resource_kind, resource_id, None, false).await;
}

pub(crate) async fn finalize_durable_dialog_as_orphan_for_epoch(
    resource_kind: &str,
    resource_id: &str,
    expected_registration_epoch_id: Option<&str>,
) {
    finalize_durable_dialog_as_orphan_inner(
        resource_kind,
        resource_id,
        expected_registration_epoch_id,
        true,
    )
    .await;
}

async fn finalize_durable_dialog_as_orphan_inner(
    resource_kind: &str,
    resource_id: &str,
    expected_registration_epoch_id: Option<&str>,
    enforce_registration_epoch: bool,
) {
    let session = match SipDialogSessionRepository::find_by_stream_id(resource_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(err) => {
            error!(
                "durable dialog force-finalize lookup failed: resource_kind={resource_kind}, resource_id={resource_id}, stage=dialog_lookup, outcome=failed, err={err}"
            );
            return;
        }
    };
    if enforce_registration_epoch
        && session.registration_epoch_id.as_deref() != expected_registration_epoch_id
    {
        debug!(
            "skip stale epoch dialog force finalize: resource_kind={resource_kind}, resource_id={resource_id}, expected_registration_epoch_id={:?}, current_registration_epoch_id={:?}",
            expected_registration_epoch_id, session.registration_epoch_id
        );
        return;
    }
    if !matches!(
        session.state,
        DialogState::Inviting | DialogState::Established | DialogState::Terminating
    ) {
        debug!(
            "durable dialog already terminal during force finalize: resource_kind={resource_kind}, resource_id={resource_id}, state={}",
            session.state
        );
        return;
    }
    match SipDialogSessionRepository::cas_mark_terminal(
        resource_id,
        &session.signal_node_id,
        session.version,
        session.state,
        DialogState::Orphan,
        "recovery_failed",
        Some("RECOVERY_FAILED"),
        Local::now().naive_local(),
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => match SipDialogSessionRepository::find_by_stream_id(resource_id).await {
            Ok(Some(current))
                if matches!(current.state, DialogState::Orphan | DialogState::Terminated) =>
            {
                debug!(
                    "durable dialog force finalize already completed: resource_kind={resource_kind}, resource_id={resource_id}, state={}",
                    current.state
                );
            }
            Ok(Some(current)) => error!(
                "durable dialog force-finalize CAS conflict: resource_kind={resource_kind}, resource_id={resource_id}, stage=dialog_cas, outcome=conflict, expected_state={}, current_state={}",
                session.state, current.state
            ),
            Ok(None) => error!(
                "durable dialog disappeared after force-finalize CAS conflict: resource_kind={resource_kind}, resource_id={resource_id}, stage=dialog_cas, outcome=missing"
            ),
            Err(err) => error!(
                "durable dialog force-finalize CAS conflict recheck failed: resource_kind={resource_kind}, resource_id={resource_id}, stage=dialog_cas_recheck, outcome=failed, err={err}"
            ),
        },
        Err(err) => error!(
            "durable dialog force finalize failed: resource_kind={resource_kind}, resource_id={resource_id}, stage=dialog_cas, outcome=failed, err={err}"
        ),
    }
}

fn release_guard_lease(lease: Option<crate::state::session::GuardLease>) {
    if let Some(lease) = lease {
        base::tokio::spawn(crate::guard_integration::release_stream_lease(lease));
    }
}

#[cfg(test)]
mod tests {
    use super::finalize_durable_dialog_as_orphan;
    use crate::storage::dialog_session::{
        DialogSessionType, DialogState, DialogTransport, SipDialogSession,
        SipDialogSessionRepository, enable_dialog_test_storage,
    };
    use base::chrono::{Duration, Local};

    #[test]
    fn force_finalizer_marks_active_durable_dialog_orphan() {
        base::tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(async {
                let _guard = enable_dialog_test_storage();
                let now = Local::now().naive_local();
                let stream_id = "force-finalizer-stream";
                SipDialogSessionRepository::insert_inviting(&SipDialogSession {
                    stream_id: stream_id.into(),
                    device_id: "34020000001320000001".into(),
                    channel_id: "34020000001320000101".into(),
                    session_type: DialogSessionType::Live,
                    signal_node_id: "session-1".into(),
                    media_node_id: "media-1".into(),
                    ssrc: Some("0100000001".into()),
                    registration_epoch_id: None,
                    call_id: "force-finalizer-call".into(),
                    local_uri: "sip:platform@127.0.0.1:5060".into(),
                    remote_uri: "sip:device@127.0.0.1:15060".into(),
                    local_tag: "force-finalizer-tag".into(),
                    remote_tag: None,
                    local_cseq: 1,
                    remote_cseq: None,
                    contact_uri: None,
                    route_set: Vec::new(),
                    local_sip_addr: "127.0.0.1:5060".into(),
                    remote_sip_addr: "127.0.0.1:15060".into(),
                    transport: DialogTransport::Udp,
                    state: DialogState::Inviting,
                    established_at: None,
                    terminated_at: None,
                    terminal_reason: None,
                    error_code: None,
                    last_seen_at: now,
                    expire_at: now + Duration::hours(1),
                    version: 0,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .expect("insert dialog");

                finalize_durable_dialog_as_orphan("stream", stream_id).await;

                assert_eq!(
                    SipDialogSessionRepository::find_by_stream_id(stream_id)
                        .await
                        .expect("find dialog")
                        .expect("dialog")
                        .state,
                    DialogState::Orphan
                );
            });
    }
}
