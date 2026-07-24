use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base::chrono::Local;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use base::tokio::time::{Instant, sleep};

use crate::gb::sip::command as sip_command;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::StreamNode;
use crate::state::session::{Cache, StreamByeCommand, StreamCloseTarget};
use crate::storage::dialog_session::{DialogState, SipDialogSessionRepository};

const CLOSE_WITHOUT_TRANSPORT_TIMEOUT: Duration = Duration::from_secs(50);
const SIP_BYE_WAIT_BUDGET: Duration = Duration::from_secs(8);
const INPUT_OBSERVATION_POLL_INTERVAL: Duration = Duration::from_millis(250);
const INPUT_OBSERVATION_MIN_TIMEOUT: Duration = Duration::from_secs(8);
const INPUT_OBSERVATION_MAX_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BeginCloseResult {
    Started,
    AlreadyClosing,
}

enum InputSilenceFailure {
    StillReceiving(String),
    Unconfirmed(String),
}

pub fn begin(stream_id: String) {
    begin_with_reason(stream_id, "session_close");
}

pub fn begin_with_reason(stream_id: String, terminal_reason: &str) {
    crate::service::playback_presence::clear_for_stream(&stream_id);
    let Some(start) = Cache::stream_close_begin(&stream_id, terminal_reason) else {
        return;
    };
    if !start.newly_started {
        retry_stream(stream_id);
        return;
    }

    let session = Register::get_device_session(&start.device_id);
    if session.is_none() && !Cache::stream_is_restored(&stream_id) {
        force_cleanup(
            &stream_id,
            start.generation,
            "device registration unavailable",
        );
        return;
    }
    let close_timeout = session
        .as_ref()
        .map(|session| session.reconnect_timeout(Instant::now()))
        .unwrap_or(CLOSE_WITHOUT_TRANSPORT_TIMEOUT)
        .max(CLOSE_WITHOUT_TRANSPORT_TIMEOUT);
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
    if session.is_none() {
        warn!(
            "restored stream close waiting for current device transport: \
             stream_id={stream_id}, device_id={}, close_timeout_secs={}",
            start.device_id,
            close_timeout.as_secs()
        );
        return;
    }
    retry_stream(stream_id);
}

pub async fn begin_manual(stream_id: String) -> GlobalResult<BeginCloseResult> {
    crate::service::playback_presence::clear_for_stream(&stream_id);
    let start = Cache::stream_close_begin(&stream_id, "manual_stop").ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "stream runtime close context not found",
            |msg| warn!("{msg}: stream_id={stream_id}"),
        )
    })?;
    let result = if start.newly_started {
        BeginCloseResult::Started
    } else {
        BeginCloseResult::AlreadyClosing
    };
    if start.newly_started
        && let Err(err) = schedule_close_deadline(&stream_id, start.generation, &start.device_id)
    {
        force_cleanup(
            &stream_id,
            start.generation,
            "schedule close deadline failed",
        );
        return Err(err);
    }

    if !start.newly_started {
        let target = Cache::stream_close_target(&stream_id).ok_or_else(|| {
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "stream close context disappeared",
                |msg| warn!("{msg}: stream_id={stream_id}"),
            )
        })?;
        quiesce_target(&stream_id, &target, "manual_stop").await?;
        retry_stream(stream_id.clone());
        return Ok(result);
    }

    let command = Cache::stream_close_take_bye(&stream_id).ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream close command is unavailable",
            |msg| warn!("{msg}: stream_id={stream_id}"),
        )
    })?;
    let pending = match sip_command::prepare_invite_stop(crate::gb::sip::InviteStopRequest {
        call_id: Some(command.call_id.clone()),
        stream_id: Some(command.stream_id.clone()),
        terminal_reason: command.terminal_reason.clone(),
    })
    .await
    {
        Ok(pending) => pending,
        Err(err) => {
            mark_failed(
                &command.stream_id,
                command.generation,
                command.seq,
                &command.device_id,
                err.to_string(),
                false,
            );
            return Err(err);
        }
    };
    let target = StreamCloseTarget {
        stream_node_name: command.stream_node_name.clone(),
        ssrc: command.ssrc,
    };
    let (node, observation) = match quiesce_target(&stream_id, &target, "manual_stop").await {
        Ok(value) => value,
        Err(err) => {
            mark_retryable_media_failed(&command, err.to_string());
            return Err(err);
        }
    };
    info!(
        "stream close accepted: stage=outputs_quiesced, outcome=stopping, operation_id=stream-close-{}-{}, device_id={}, stream_id={}, ssrc={}, call_id={}, generation={}, stream_lifecycle_generation={}, packet_count={}",
        command.stream_id,
        command.generation,
        command.device_id,
        command.stream_id,
        command.ssrc,
        command.call_id,
        command.generation,
        observation.lifecycle_generation,
        observation.packet_count
    );
    base::tokio::spawn(finish_staged_close(command, pending, node, observation));
    Ok(result)
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
    let pending = match sip_command::prepare_invite_stop(crate::gb::sip::InviteStopRequest {
        call_id: Some(command.call_id.clone()),
        stream_id: Some(command.stream_id.clone()),
        terminal_reason: command.terminal_reason.clone(),
    })
    .await
    {
        Ok(pending) => pending,
        Err(err) => {
            mark_failed(
                &command.stream_id,
                command.generation,
                command.seq,
                &command.device_id,
                err.to_string(),
                false,
            );
            return;
        }
    };
    let target = StreamCloseTarget {
        stream_node_name: command.stream_node_name.clone(),
        ssrc: command.ssrc,
    };
    let (node, observation) =
        match quiesce_target(&command.stream_id, &target, &command.terminal_reason).await {
            Ok(value) => value,
            Err(err) => {
                mark_retryable_media_failed(&command, err.to_string());
                return;
            }
        };
    finish_staged_close(command, pending, node, observation).await;
}

async fn quiesce_target(
    stream_id: &str,
    target: &StreamCloseTarget,
    reason: &str,
) -> GlobalResult<(
    StreamNode,
    crate::service::stream_rpc::StreamInputObservation,
)> {
    if target.stream_node_name.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream media node is missing",
            |msg| warn!("{msg}: stream_id={stream_id}"),
        ));
    }
    let node = crate::guard_integration::ensure_stream_node(&target.stream_node_name).await?;
    let observation = crate::service::stream_rpc::quiesce_receive_outputs(
        &node,
        stream_id,
        target.ssrc,
        0,
        reason,
    )
    .await?;
    Ok((node, observation))
}

async fn finish_staged_close(
    command: StreamByeCommand,
    pending: sip_command::PendingInviteStop,
    node: StreamNode,
    observation: crate::service::stream_rpc::StreamInputObservation,
) {
    let silence_deadline = Instant::now()
        + SIP_BYE_WAIT_BUDGET
        + input_observation_timeout(Duration::from_millis(
            observation.input_idle_timeout_ms.max(1),
        ));
    let bye = sip_command::send_prepared_invite_stop(&command.device_id, &pending);
    let silence = wait_for_input_silence(&node, &command, observation, silence_deadline);
    let (bye_result, silence_result) = base::tokio::join!(bye, silence);

    if let Err(err) = bye_result {
        mark_failed(
            &command.stream_id,
            command.generation,
            command.seq,
            &command.device_id,
            err.to_string(),
            false,
        );
        return;
    }
    let mut silent = match silence_result {
        Ok(observation) => observation,
        Err(InputSilenceFailure::StillReceiving(reason)) => {
            mark_terminal_failed(
                &command,
                reason,
                "media_still_receiving",
                "MEDIA_STILL_RECEIVING",
            );
            return;
        }
        Err(InputSilenceFailure::Unconfirmed(reason)) => {
            mark_terminal_failed(
                &command,
                reason,
                "media_close_unconfirmed",
                "MEDIA_CLOSE_UNCONFIRMED",
            );
            return;
        }
    };
    info!(
        "stream close evidence confirmed: stage=bye_and_input_silence, outcome=confirmed, operation_id=stream-close-{}-{}, device_id={}, stream_id={}, ssrc={}, call_id={}, generation={}, stream_lifecycle_generation={}, last_packet_at_ms={}, packet_count={}, idle_timeout_ms={}",
        command.stream_id,
        command.generation,
        command.device_id,
        command.stream_id,
        command.ssrc,
        command.call_id,
        command.generation,
        silent.lifecycle_generation,
        silent.last_packet_at_ms,
        silent.packet_count,
        silent.input_idle_timeout_ms
    );
    loop {
        match crate::service::stream_rpc::finalize_receive(
            &node,
            &command.stream_id,
            command.ssrc,
            silent.lifecycle_generation,
            silent.packet_count,
            &command.terminal_reason,
        )
        .await
        {
            Ok(crate::service::stream_rpc::FinalizeReceiveResult::Finalized(_)) => break,
            Ok(crate::service::stream_rpc::FinalizeReceiveResult::InputChanged(latest)) => {
                silent =
                    match wait_for_input_silence(&node, &command, latest, silence_deadline).await {
                        Ok(observation) => observation,
                        Err(InputSilenceFailure::StillReceiving(reason)) => {
                            mark_terminal_failed(
                                &command,
                                reason,
                                "media_still_receiving",
                                "MEDIA_STILL_RECEIVING",
                            );
                            return;
                        }
                        Err(InputSilenceFailure::Unconfirmed(reason)) => {
                            mark_terminal_failed(
                                &command,
                                reason,
                                "media_close_unconfirmed",
                                "MEDIA_CLOSE_UNCONFIRMED",
                            );
                            return;
                        }
                    };
            }
            Err(err) => {
                mark_terminal_failed(
                    &command,
                    err.to_string(),
                    "media_close_unconfirmed",
                    "MEDIA_CLOSE_UNCONFIRMED",
                );
                return;
            }
        }
    }
    if let Err(err) = sip_command::complete_invite_stop(pending).await {
        mark_terminal_failed(
            &command,
            err.to_string(),
            "internal_error",
            "INTERNAL_ERROR",
        );
        return;
    }
    if let Some(info) = Cache::stream_close_complete(&command.stream_id, command.generation) {
        info!(
            "stream close completed: stage=close_finalize, outcome=closed, operation_id=stream-close-{}-{}, device_id={}, channel_id={}, stream_id={}, ssrc={}, call_id={}, generation={}, bye_confirmed=true, input_silent=true, stream_finalized=true",
            info.stream_id,
            info.generation,
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

async fn wait_for_input_silence(
    node: &StreamNode,
    command: &StreamByeCommand,
    initial: crate::service::stream_rpc::StreamInputObservation,
    deadline: Instant,
) -> Result<crate::service::stream_rpc::StreamInputObservation, InputSilenceFailure> {
    let mut latest = initial;
    loop {
        let now_ms = unix_time_ms();
        if input_is_silent(now_ms, &latest) {
            return Ok(latest);
        }
        if Instant::now() >= deadline {
            return Err(InputSilenceFailure::StillReceiving(format!(
                "SSRC did not become silent before observation deadline: stream_id={}, ssrc={}, last_packet_at_ms={}, packet_count={}, idle_timeout_ms={}",
                command.stream_id,
                command.ssrc,
                latest.last_packet_at_ms,
                latest.packet_count,
                latest.input_idle_timeout_ms
            )));
        }
        sleep(INPUT_OBSERVATION_POLL_INTERVAL).await;
        latest = crate::service::stream_rpc::query_input_observation(
            node,
            &command.stream_id,
            command.ssrc,
        )
        .await
        .map_err(|err| InputSilenceFailure::Unconfirmed(err.to_string()))?;
        if latest.lifecycle_generation != initial.lifecycle_generation {
            return Err(InputSilenceFailure::Unconfirmed(format!(
                "stream lifecycle generation changed while observing input: expected={}, actual={}",
                initial.lifecycle_generation, latest.lifecycle_generation
            )));
        }
    }
}

fn input_is_silent(
    now_ms: u64,
    observation: &crate::service::stream_rpc::StreamInputObservation,
) -> bool {
    now_ms.saturating_sub(observation.last_packet_at_ms) >= observation.input_idle_timeout_ms.max(1)
}

fn input_observation_timeout(idle_timeout: Duration) -> Duration {
    idle_timeout
        .saturating_mul(3)
        .max(INPUT_OBSERVATION_MIN_TIMEOUT)
        .min(INPUT_OBSERVATION_MAX_TIMEOUT)
}

fn schedule_close_deadline(stream_id: &str, generation: u64, device_id: &str) -> GlobalResult<()> {
    let close_timeout = Register::get_device_session(device_id)
        .map(|session| session.reconnect_timeout(Instant::now()))
        .unwrap_or(CLOSE_WITHOUT_TRANSPORT_TIMEOUT)
        .max(CLOSE_WITHOUT_TRANSPORT_TIMEOUT);
    Register::scheduler()
        .insert_register(
            TimeScheduleKey::StreamClosing(Arc::from(stream_id), generation),
            close_timeout,
        )
        .map_err(|err| {
            GlobalError::new_sys_error("schedule stream close deadline failed", |msg| {
                error!("{msg}: stream_id={stream_id}, generation={generation}, err={err}")
            })
        })
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

fn mark_terminal_failed(
    command: &StreamByeCommand,
    reason: String,
    terminal_reason: &str,
    error_code: &str,
) {
    if Cache::stream_close_mark_terminal_failure(
        &command.stream_id,
        command.generation,
        command.seq,
        reason.clone(),
        terminal_reason,
        error_code,
    ) {
        warn!(
            "stream close awaiting forced terminalization: stream_id={}, generation={}, cseq={}, terminal_reason={}, error_code={}, reason={}",
            command.stream_id, command.generation, command.seq, terminal_reason, error_code, reason
        );
    }
}

fn mark_retryable_media_failed(command: &StreamByeCommand, reason: String) {
    if Cache::stream_close_mark_retryable_failure(
        &command.stream_id,
        command.generation,
        command.seq,
        reason.clone(),
        "media_close_unconfirmed",
        "MEDIA_CLOSE_UNCONFIRMED",
    ) {
        warn!(
            "stream output quiesce pending retry: stream_id={}, generation={}, cseq={}, reason={}",
            command.stream_id, command.generation, command.seq, reason
        );
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
            let _ = stop_media_runtime(&stream_id, &stream_node_name, "force_cleanup").await;
            finalize_stream_close_as_orphan(
                &stream_id,
                info.failure_terminal_reason
                    .as_deref()
                    .unwrap_or("close_timeout"),
                info.failure_error_code
                    .as_deref()
                    .unwrap_or("SIP_BYE_TIMEOUT"),
            )
            .await;
        });
    } else {
        debug!(
            "ignore stale stream force cleanup: stream_id={}, generation={}",
            stream_id, generation
        );
    }
}

pub(crate) async fn stop_media_runtime(
    stream_id: &str,
    stream_node_name: &str,
    reason: &str,
) -> base::exception::GlobalResult<()> {
    if stream_node_name.is_empty() {
        return Err(base::exception::GlobalError::new_biz_error(
            base::err::BaseErrorCode::InvalidState.code(),
            "stream media node is missing",
            |msg| warn!("{msg}: stream_id={stream_id}, reason={reason}"),
        ));
    }
    let node = match crate::guard_integration::ensure_stream_node(stream_node_name).await {
        Ok(node) => node,
        Err(err) => {
            warn!(
                "stream media cleanup deferred: stage=resolve_stream_node, outcome=unavailable, stream_id={}, stream_node={}, reason={}, error={}",
                stream_id, stream_node_name, reason, err
            );
            return Err(err);
        }
    };
    let result = crate::service::stream_rpc::stop_receive(&node, stream_id, reason).await;
    if let Err(err) = &result {
        warn!(
            "stream media cleanup failed: stage=stop_receive, outcome=failed, stream_id={}, stream_node={}, reason={}, error={}",
            stream_id, stream_node_name, reason, err
        );
    }
    result
}

pub(crate) async fn finalize_durable_dialog_as_orphan(resource_kind: &str, resource_id: &str) {
    finalize_durable_dialog_as_orphan_inner(
        resource_kind,
        resource_id,
        None,
        false,
        "recovery_failed",
        "RECOVERY_FAILED",
    )
    .await;
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
        "recovery_failed",
        "RECOVERY_FAILED",
    )
    .await;
}

pub(crate) async fn close_unlinked_inviting_stream(
    session: &crate::storage::dialog_session::SipDialogSession,
) -> GlobalError {
    let media_result = stop_media_runtime(
        &session.stream_id,
        &session.media_node_id,
        "manual_stop_unlinked_inviting",
    )
    .await;
    let sip_result = match sip_command::prepare_invite_stop(crate::gb::sip::InviteStopRequest {
        call_id: Some(session.call_id.clone()),
        stream_id: Some(session.stream_id.clone()),
        terminal_reason: "manual_stop".to_string(),
    })
    .await
    {
        Ok(pending) => sip_command::send_prepared_invite_stop(&session.device_id, &pending).await,
        Err(err) => Err(err),
    };
    if let Err(err) = &media_result {
        warn!(
            "unlinked INVITING stream media cleanup failed: stream_id={}, err={err}",
            session.stream_id
        );
    }
    if let Err(err) = &sip_result {
        warn!(
            "unlinked INVITING stream SIP cleanup failed: stream_id={}, err={err}",
            session.stream_id
        );
    }
    finalize_stream_close_as_orphan(&session.stream_id, "linkage_failed", "LINKAGE_FAILED").await;
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "stream runtime linkage was incomplete; cleanup was recorded as abnormal",
        |msg| warn!("{msg}: stream_id={}", session.stream_id),
    )
}

async fn finalize_stream_close_as_orphan(stream_id: &str, terminal_reason: &str, error_code: &str) {
    finalize_durable_dialog_as_orphan_inner(
        "stream",
        stream_id,
        None,
        false,
        terminal_reason,
        error_code,
    )
    .await;
}

async fn finalize_durable_dialog_as_orphan_inner(
    resource_kind: &str,
    resource_id: &str,
    expected_registration_epoch_id: Option<&str>,
    enforce_registration_epoch: bool,
    terminal_reason: &str,
    error_code: &str,
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
        terminal_reason,
        Some(error_code),
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
    use super::{finalize_stream_close_as_orphan, input_is_silent, input_observation_timeout};
    use crate::service::stream_rpc::StreamInputObservation;
    use crate::storage::dialog_session::{
        DialogSessionType, DialogState, DialogTransport, SipDialogSession,
        SipDialogSessionRepository, enable_dialog_test_storage,
    };
    use base::chrono::{Duration, Local};
    use std::time::Duration as StdDuration;

    #[test]
    fn input_silence_requires_a_full_idle_window_after_the_latest_packet() {
        let mut observation = StreamInputObservation {
            ssrc: 200_000_011,
            lifecycle_generation: 7,
            last_packet_at_ms: 1_000,
            packet_count: 10,
            input_idle_timeout_ms: 4_000,
        };

        assert!(!input_is_silent(4_999, &observation));
        assert!(input_is_silent(5_000, &observation));

        observation.last_packet_at_ms = 4_900;
        observation.packet_count += 1;
        assert!(!input_is_silent(5_000, &observation));
        assert!(input_is_silent(8_900, &observation));
    }

    #[test]
    fn input_observation_deadline_is_bounded() {
        assert_eq!(
            input_observation_timeout(StdDuration::from_secs(1)),
            StdDuration::from_secs(8)
        );
        assert_eq!(
            input_observation_timeout(StdDuration::from_secs(4)),
            StdDuration::from_secs(12)
        );
        assert_eq!(
            input_observation_timeout(StdDuration::from_secs(60)),
            StdDuration::from_secs(30)
        );
    }

    #[test]
    fn stream_close_finalizer_records_orphan_failure() {
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

                finalize_stream_close_as_orphan(
                    stream_id,
                    "media_still_receiving",
                    "MEDIA_STILL_RECEIVING",
                )
                .await;

                let dialog = SipDialogSessionRepository::find_by_stream_id(stream_id)
                    .await
                    .expect("find dialog")
                    .expect("dialog");
                assert_eq!(dialog.state, DialogState::Orphan);
                assert_eq!(
                    dialog.terminal_reason.as_deref(),
                    Some("media_still_receiving")
                );
                assert_eq!(dialog.error_code.as_deref(), Some("MEDIA_STILL_RECEIVING"));
            });
    }
}
