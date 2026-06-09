use std::sync::Arc;

use base::exception::GlobalResult;
use base::log::{debug, error, warn};
use base::tokio::time::Instant;

use crate::gb::depot::Callback;
use crate::gb::handler::cmd::CmdStream;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, StreamByeCommand};

pub fn begin(stream_id: String) {
    let Some(start) = Cache::stream_close_begin(&stream_id) else {
        return;
    };
    if !start.newly_started {
        return;
    }

    let Some(session) = Register::get_device_session(&start.device_id) else {
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
    let callback_stream_id = command.stream_id.clone();
    let callback_generation = command.generation;
    let callback_seq = command.seq;
    let callback_device_id = command.device_id.clone();
    let callback: Callback = Box::new(move |result| {
        handle_bye_result(
            &callback_stream_id,
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
            &command.stream_id,
            command.generation,
            command.seq,
            &command.device_id,
            err.to_string(),
            false,
        );
    }
}

fn handle_bye_result(
    stream_id: &str,
    generation: u64,
    seq: u32,
    device_id: &str,
    result: GlobalResult<rsip::Response>,
) {
    match result {
        Ok(response) => {
            let status = response.status_code.code();
            if is_bye_terminal_status(status) {
                if let Some(info) = Cache::stream_close_complete(stream_id, generation) {
                    debug!(
                        "stream close completed: device_id={}, channel_id={}, stream_id={}, \
                         ssrc={}, call_id={}, status={}",
                        info.device_id,
                        info.channel_id,
                        info.stream_id,
                        info.ssrc,
                        info.call_id,
                        status
                    );
                }
            } else {
                mark_failed(
                    stream_id,
                    generation,
                    seq,
                    device_id,
                    format!("unexpected BYE response: {status}"),
                    false,
                );
            }
        }
        Err(err) => mark_failed(
            stream_id,
            generation,
            seq,
            device_id,
            err.to_string(),
            true,
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
            "force cleanup closing stream: device_id={}, channel_id={}, stream_id={}, \
             ssrc={}, call_id={}, generation={}, reason={}",
            info.device_id,
            info.channel_id,
            info.stream_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason
        );
    }
}

fn is_bye_terminal_status(status: u16) -> bool {
    (200..300).contains(&status) || status == 481
}

#[cfg(test)]
mod tests {
    use super::is_bye_terminal_status;

    #[test]
    fn bye_2xx_and_481_are_terminal() {
        assert!(is_bye_terminal_status(200));
        assert!(is_bye_terminal_status(299));
        assert!(is_bye_terminal_status(481));
    }

    #[test]
    fn other_bye_responses_keep_closing_state() {
        assert!(!is_bye_terminal_status(300));
        assert!(!is_bye_terminal_status(408));
        assert!(!is_bye_terminal_status(500));
    }
}
