use std::sync::Arc;

use base::chrono::Local;
use base::log::{debug, error, warn};
use base::tokio::time::Instant;

use crate::gb::sip::command as sip_command;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, StreamByeCommand};
use crate::storage::dialog_session::{DialogState, SipDialogSessionRepository};

pub fn begin(stream_id: String) {
    let Some(start) = Cache::stream_close_begin(&stream_id) else {
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
    let result = sip_command::invite_stop_by_device(
        &command.device_id,
        crate::gb::sip::InviteStopRequest {
            call_id: Some(command.call_id.clone()),
            stream_id: Some(command.stream_id.clone()),
        },
    )
    .await;

    match result {
        Ok(()) => {
            if let Some(info) = Cache::stream_close_complete(&stream_id, generation) {
                debug!(
                    "stream close completed: device_id={}, channel_id={}, stream_id={}, ssrc={}, call_id={}",
                    info.device_id, info.channel_id, info.stream_id, info.ssrc, info.call_id
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

fn force_cleanup(stream_id: &str, generation: u64, reason: &str) {
    if let Some(info) = Cache::stream_close_force(stream_id, generation) {
        error!(
            "force cleanup closing stream: device_id={}, channel_id={}, stream_id={}, ssrc={}, call_id={}, generation={}, reason={}",
            info.device_id,
            info.channel_id,
            info.stream_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason
        );
        release_guard_lease(info.guard_lease);
        let stream_id = info.stream_id;
        base::tokio::spawn(async move {
            let Ok(Some(session)) = SipDialogSessionRepository::find_by_stream_id(&stream_id).await
            else {
                return;
            };
            if matches!(
                session.state,
                DialogState::Inviting | DialogState::Established | DialogState::Terminating
            ) {
                let _ = SipDialogSessionRepository::cas_transition(
                    &stream_id,
                    &session.signal_node_id,
                    session.version,
                    session.state,
                    DialogState::Orphan,
                    Local::now().naive_local(),
                )
                .await;
            }
        });
    }
}

fn release_guard_lease(lease: Option<crate::state::session::GuardLease>) {
    if let Some(lease) = lease {
        base::tokio::spawn(crate::guard_integration::release_stream_lease(lease));
    }
}
