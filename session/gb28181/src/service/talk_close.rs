use std::sync::Arc;

use base::log::{debug, error, warn};
use base::tokio::time::Instant;

use crate::gb::sip::command as sip_command;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, TalkByeCommand};

pub fn begin(talk_id: String) -> bool {
    let Some(start) = Cache::talk_close_begin(&talk_id) else {
        return false;
    };
    if !start.newly_started {
        return true;
    }

    let Some(session) = Register::get_device_session(&start.device_id) else {
        force_cleanup(
            &talk_id,
            start.generation,
            "device registration unavailable",
        );
        return true;
    };
    let close_timeout = session.reconnect_timeout(Instant::now());
    if close_timeout.is_zero() {
        force_cleanup(&talk_id, start.generation, "close deadline expired");
        return true;
    }
    if let Err(err) = Register::scheduler().insert_register(
        TimeScheduleKey::TalkClosing(Arc::from(talk_id.as_str()), start.generation),
        close_timeout,
    ) {
        force_cleanup(
            &talk_id,
            start.generation,
            &format!("schedule close deadline failed: {err}"),
        );
        return true;
    }
    retry_talk(talk_id);
    true
}

pub fn retry_device(device_id: &str) {
    for talk_id in Cache::talk_close_ids_by_device(device_id) {
        retry_talk(talk_id);
    }
}

fn retry_talk(talk_id: String) {
    let Some(command) = Cache::talk_close_take_bye(&talk_id) else {
        return;
    };
    base::tokio::spawn(send_bye(command));
}

async fn send_bye(command: TalkByeCommand) {
    let talk_id = command.talk_id.clone();
    let generation = command.generation;
    let seq = command.seq;
    let device_id = command.device_id.clone();
    let result = sip_command::invite_stop_by_device(
        &command.device_id,
        crate::gb::sip::InviteStopRequest {
            call_id: Some(command.call_id.clone()),
            stream_id: Some(command.talk_id.clone()),
        },
    )
    .await;

    match result {
        Ok(()) => {
            if let Some(info) = Cache::talk_close_complete(&talk_id, generation) {
                debug!(
                    "talk close completed: device_id={}, channel_id={}, talk_id={}, ssrc={}, call_id={}",
                    info.device_id, info.channel_id, info.talk_id, info.ssrc, info.call_id
                );
                release_guard_lease(info.guard_lease);
            }
        }
        Err(err) => mark_failed(
            &talk_id,
            generation,
            seq,
            &device_id,
            err.to_string(),
            false,
        ),
    }
}

fn mark_failed(
    talk_id: &str,
    generation: u64,
    seq: u32,
    device_id: &str,
    reason: String,
    retry_if_connected: bool,
) {
    if Cache::talk_close_mark_failed(talk_id, generation, seq, reason.clone()) {
        warn!(
            "talk BYE pending retry: talk_id={}, generation={}, cseq={}, reason={}",
            talk_id, generation, seq, reason
        );
        if retry_if_connected && Register::get_connected_device_session(device_id).is_some() {
            retry_talk(talk_id.to_string());
        }
    }
}

fn force_cleanup(talk_id: &str, generation: u64, reason: &str) {
    if let Some(info) = Cache::talk_close_force(talk_id, generation) {
        error!(
            "force cleanup closing talk: device_id={}, channel_id={}, talk_id={}, ssrc={}, call_id={}, generation={}, reason={}",
            info.device_id,
            info.channel_id,
            info.talk_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason
        );
        release_guard_lease(info.guard_lease);
    }
}

fn release_guard_lease(lease: Option<crate::state::session::GuardLease>) {
    if let Some(lease) = lease {
        base::tokio::spawn(crate::guard_integration::release_stream_lease(lease));
    }
}
