use std::sync::Arc;

use base::log::{debug, info, warn};
use base::tokio::time::Instant;

use crate::gb::sip::command as sip_command;
use crate::register::core::{Register, TimeScheduleKey};
use crate::service::stream_close::finalize_durable_dialog_as_orphan;
use crate::state::session::{BroadcastByeCommand, Cache};

pub fn begin(broadcast_id: String) -> bool {
    begin_with_reason(broadcast_id, "session_close")
}

pub fn begin_with_reason(broadcast_id: String, terminal_reason: &str) -> bool {
    let Some(start) = Cache::broadcast_close_begin(&broadcast_id, terminal_reason) else {
        return false;
    };
    if !start.newly_started {
        return true;
    }

    let Some(session) = Register::get_device_session(&start.device_id) else {
        force_cleanup(
            &broadcast_id,
            start.generation,
            "device registration unavailable",
        );
        return true;
    };
    let close_timeout = session.reconnect_timeout(Instant::now());
    if close_timeout.is_zero() {
        force_cleanup(&broadcast_id, start.generation, "close deadline expired");
        return true;
    }
    if let Err(err) = Register::scheduler().insert_register(
        TimeScheduleKey::BroadcastClosing(Arc::from(broadcast_id.as_str()), start.generation),
        close_timeout,
    ) {
        force_cleanup(
            &broadcast_id,
            start.generation,
            &format!("schedule close deadline failed: {err}"),
        );
        return true;
    }
    retry_broadcast(broadcast_id);
    true
}

pub fn retry_device(device_id: &str) {
    for broadcast_id in Cache::broadcast_close_ids_by_device(device_id) {
        retry_broadcast(broadcast_id);
    }
}

fn retry_broadcast(broadcast_id: String) {
    let Some(command) = Cache::broadcast_close_take_bye(&broadcast_id) else {
        return;
    };
    base::tokio::spawn(send_bye(command));
}

async fn send_bye(command: BroadcastByeCommand) {
    let broadcast_id = command.broadcast_id.clone();
    let generation = command.generation;
    let seq = command.seq;
    let device_id = command.device_id.clone();
    let result = sip_command::invite_stop_by_device(
        &command.device_id,
        crate::gb::sip::InviteStopRequest {
            call_id: Some(command.call_id.clone()),
            stream_id: Some(command.broadcast_id.clone()),
            terminal_reason: command.terminal_reason,
        },
    )
    .await;

    match result {
        Ok(()) => {
            if let Some(info) = Cache::broadcast_close_complete(&broadcast_id, generation) {
                info!(
                    "broadcast close completed: stage=close_finalize, outcome=closed, device_id={}, channel_id={}, broadcast_id={}, ssrc={}, call_id={}, generation={}",
                    info.device_id,
                    info.channel_id,
                    info.broadcast_id,
                    info.ssrc,
                    info.call_id,
                    info.generation
                );
                cleanup_broadcast_leg(
                    info.stream_node_name,
                    info.parent_broadcast_id,
                    info.broadcast_id.clone(),
                );
                release_guard_lease(info.guard_lease);
            }
        }
        Err(err) => mark_failed(
            &broadcast_id,
            generation,
            seq,
            &device_id,
            err.to_string(),
            false,
        ),
    }
}

fn mark_failed(
    broadcast_id: &str,
    generation: u64,
    seq: u32,
    device_id: &str,
    reason: String,
    retry_if_connected: bool,
) {
    if Cache::broadcast_close_mark_failed(broadcast_id, generation, seq, reason.clone()) {
        warn!(
            "broadcast BYE pending retry: broadcast_id={}, generation={}, cseq={}, reason={}",
            broadcast_id, generation, seq, reason
        );
        if retry_if_connected && Register::get_connected_device_session(device_id).is_some() {
            retry_broadcast(broadcast_id.to_string());
        }
    }
}

pub(crate) fn force_cleanup(broadcast_id: &str, generation: u64, reason: &str) {
    if let Some(info) = Cache::broadcast_close_force(broadcast_id, generation) {
        warn!(
            "broadcast close forced: stage=force_finalize, outcome=forced, device_id={}, channel_id={}, broadcast_id={}, ssrc={}, call_id={}, generation={}, trigger_reason={}, last_error={}",
            info.device_id,
            info.channel_id,
            info.broadcast_id,
            info.ssrc,
            info.call_id,
            info.generation,
            reason,
            info.last_error.as_deref().unwrap_or("none")
        );
        release_guard_lease(info.guard_lease);
        let broadcast_id = info.broadcast_id;
        cleanup_broadcast_leg(
            info.stream_node_name,
            info.parent_broadcast_id,
            broadcast_id.clone(),
        );
        base::tokio::spawn(async move {
            finalize_durable_dialog_as_orphan("broadcast", &broadcast_id).await;
        });
    } else {
        debug!(
            "ignore stale broadcast force cleanup: broadcast_id={}, generation={}",
            broadcast_id, generation
        );
    }
}

fn release_guard_lease(lease: Option<crate::state::session::GuardLease>) {
    if let Some(lease) = lease {
        base::tokio::spawn(crate::guard_integration::release_stream_lease(lease));
    }
}

fn cleanup_broadcast_leg(stream_node_name: String, parent_id: String, leg_id: String) {
    base::tokio::spawn(async move {
        let Ok(node) = crate::guard_integration::ensure_stream_node(&stream_node_name).await else {
            return;
        };
        if let Err(error) =
            crate::service::stream_rpc::broadcast_close(&node, &parent_id, &leg_id).await
        {
            warn!(
                "broadcast media cleanup failed: broadcast_id={parent_id}, leg_id={leg_id}, reason={error}"
            );
        }
    });
}
