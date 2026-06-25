use crate::core::{GuardError, GuardResult};
use crate::store::model::{OutboxRecord, OutboxState};

pub fn mark_sending(record: &mut OutboxRecord, now_ms: i64) -> GuardResult<()> {
    if !matches!(record.state, OutboxState::Pending | OutboxState::RetryWait) {
        return Err(GuardError::Conflict(format!(
            "outbox {} cannot start from {:?}",
            record.outbox_id, record.state
        )));
    }
    record.state = OutboxState::Sending;
    record.attempts = record.attempts.saturating_add(1);
    record.updated_at_ms = now_ms;
    Ok(())
}

pub fn mark_delivered(record: &mut OutboxRecord, now_ms: i64) -> GuardResult<()> {
    require_sending(record)?;
    record.state = OutboxState::Delivered;
    record.last_error = None;
    record.updated_at_ms = now_ms;
    Ok(())
}

pub fn mark_retry(
    record: &mut OutboxRecord,
    now_ms: i64,
    next_attempt_at_ms: i64,
    error: impl Into<String>,
) -> GuardResult<()> {
    require_sending(record)?;
    record.state = OutboxState::RetryWait;
    record.next_attempt_at_ms = next_attempt_at_ms;
    record.last_error = Some(error.into());
    record.updated_at_ms = now_ms;
    Ok(())
}

pub fn mark_dead(
    record: &mut OutboxRecord,
    now_ms: i64,
    error: impl Into<String>,
) -> GuardResult<()> {
    require_sending(record)?;
    record.state = OutboxState::Dead;
    record.last_error = Some(error.into());
    record.updated_at_ms = now_ms;
    Ok(())
}

fn require_sending(record: &OutboxRecord) -> GuardResult<()> {
    if record.state != OutboxState::Sending {
        return Err(GuardError::Conflict(format!(
            "outbox {} is not sending",
            record.outbox_id
        )));
    }
    Ok(())
}
