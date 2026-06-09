use std::sync::Arc;

use base::exception::GlobalResult;
use base::log::{debug, error, warn};
use base::tokio::time::Instant;

use crate::gb::depot::Callback;
use crate::gb::handler::cmd::CmdStream;
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
            &format!("schedule talk close deadline failed: {err}"),
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
    let callback_talk_id = command.talk_id.clone();
    let callback_generation = command.generation;
    let callback_seq = command.seq;
    let callback_device_id = command.device_id.clone();
    let callback: Callback = Box::new(move |result| {
        handle_bye_result(
            &callback_talk_id,
            callback_generation,
            callback_seq,
            &callback_device_id,
            result,
        );
    });
    let result = CmdStream::play_bye_with_callback(
        command.seq,
        command.call_id,
        &command.device_id,
        &command.remote_target,
        &command.route_set,
        &command.from_header,
        &command.to_header,
        callback,
    )
    .await;
    if let Err(err) = result {
        mark_failed(
            &command.talk_id,
            command.generation,
            command.seq,
            &command.device_id,
            err.to_string(),
            false,
        );
    }
}

fn handle_bye_result(
    talk_id: &str,
    generation: u64,
    seq: u32,
    device_id: &str,
    result: GlobalResult<rsip::Response>,
) {
    match result {
        Ok(response) => {
            let status = response.status_code.code();
            if (200..300).contains(&status) || status == 481 {
                if let Some(info) = Cache::talk_close_complete(talk_id, generation) {
                    debug!(
                        "talk close completed: device_id={}, channel_id={}, talk_id={}, \
                         ssrc={}, call_id={}, status={}",
                        info.device_id,
                        info.channel_id,
                        info.talk_id,
                        info.ssrc,
                        info.call_id,
                        status
                    );
                }
            } else {
                mark_failed(
                    talk_id,
                    generation,
                    seq,
                    device_id,
                    format!("unexpected talk BYE response: {status}"),
                    false,
                );
            }
        }
        Err(err) => mark_failed(
            talk_id,
            generation,
            seq,
            device_id,
            err.to_string(),
            true,
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
            "force cleanup closing talk: device_id={}, channel_id={}, talk_id={}, \
             ssrc={}, call_id={}, generation={}, reason={}",
            info.device_id,
            info.channel_id,
            info.talk_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason
        );
    }
}
