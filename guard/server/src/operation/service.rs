use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::auth::Role;
use crate::core::{GuardError, GuardResult};
use crate::operation::state::{OperationRecord, OperationStatus};

const TERMINAL_RETENTION_MS: i64 = 60 * 60 * 1_000;
const MAX_OPERATION_RECORDS: usize = 10_000;

#[derive(Debug, Clone)]
pub struct OperationRequest {
    pub operation_id: String,
    pub kind: String,
    pub requested_by: String,
    pub caller_role: Role,
    pub required_role: Role,
    pub dangerous: bool,
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OperationService {
    records: Arc<Mutex<HashMap<String, OperationRecord>>>,
}

impl OperationService {
    pub fn start(&self, request: OperationRequest) -> GuardResult<OperationRecord> {
        self.start_once(request).map(|(record, _)| record)
    }

    pub fn start_once(&self, request: OperationRequest) -> GuardResult<(OperationRecord, bool)> {
        if request.operation_id.is_empty() || request.kind.is_empty() {
            return Err(GuardError::InvalidConfig(
                "operation_id and kind are required".to_string(),
            ));
        }
        if !request.caller_role.allows(request.required_role) {
            return Err(GuardError::InvalidIdentity(
                "caller role is not allowed to start operation".to_string(),
            ));
        }
        if request.dangerous && request.confirmation.as_deref() != Some(request.kind.as_str()) {
            return Err(GuardError::InvalidConfig(
                "dangerous operation requires matching confirmation".to_string(),
            ));
        }

        let now_ms = now_ms();
        let record = OperationRecord {
            operation_id: request.operation_id,
            kind: request.kind,
            requested_by: request.requested_by,
            required_role: request.required_role,
            status: OperationStatus::Accepted,
            progress_percent: 0,
            stage: "accepted".to_string(),
            message: String::new(),
            error: None,
            result: None,
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            checkpoint_ms: 0,
            hard_timeout_ms: 0,
        };
        let mut records = self.records.lock();
        prune_records(&mut records, now_ms);
        if let Some(existing) = records.get(&record.operation_id) {
            if existing.kind == record.kind && existing.requested_by == record.requested_by {
                base::log::debug!(
                    "operation request reused: action=operation, stage=accepted, outcome=duplicate, operation_id={}, kind={}, requested_by={}",
                    existing.operation_id,
                    existing.kind,
                    existing.requested_by
                );
                return Ok((existing.clone(), false));
            }
            return Err(GuardError::Conflict(format!(
                "operation {} already exists",
                record.operation_id
            )));
        }
        records.insert(record.operation_id.clone(), record.clone());
        base::log::info!(
            "operation accepted: action=operation, stage=accepted, outcome=accepted, operation_id={}, kind={}, requested_by={}",
            record.operation_id,
            record.kind,
            record.requested_by
        );
        Ok((record, true))
    }

    pub fn progress(
        &self,
        operation_id: &str,
        progress_percent: u8,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        if progress_percent > 100 {
            return Err(GuardError::InvalidConfig(
                "operation progress must be <= 100".to_string(),
            ));
        }
        self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Running;
            record.progress_percent = progress_percent;
            record.message = message.into();
            record.updated_at_ms = now_ms();
            Ok(())
        })
    }

    pub fn configure_media(
        &self,
        operation_id: &str,
        stage: impl Into<String>,
        checkpoint_ms: u64,
        hard_timeout_ms: u64,
    ) -> GuardResult<OperationRecord> {
        if checkpoint_ms == 0 || hard_timeout_ms < checkpoint_ms {
            return Err(GuardError::InvalidConfig(
                "media operation timeout budget is invalid".to_string(),
            ));
        }
        self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Running;
            record.stage = stage.into();
            record.checkpoint_ms = checkpoint_ms;
            record.hard_timeout_ms = hard_timeout_ms;
            record.updated_at_ms = now_ms();
            Ok(())
        })
    }

    pub fn progress_stage(
        &self,
        operation_id: &str,
        stage: impl Into<String>,
        progress_percent: u8,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        if progress_percent > 100 {
            return Err(GuardError::InvalidConfig(
                "operation progress must be <= 100".to_string(),
            ));
        }
        self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Running;
            record.stage = stage.into();
            record.progress_percent = progress_percent;
            record.message = message.into();
            record.updated_at_ms = now_ms();
            Ok(())
        })
    }

    pub fn succeed(
        &self,
        operation_id: &str,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        let record = self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Succeeded;
            record.progress_percent = 100;
            record.stage = "ready".to_string();
            record.message = message.into();
            record.updated_at_ms = now_ms();
            Ok(())
        })?;
        base::log::info!(
            "operation completed: action=operation, stage=terminal, outcome=succeeded, operation_id={}, kind={}, elapsed_ms={}",
            record.operation_id,
            record.kind,
            record.updated_at_ms.saturating_sub(record.started_at_ms)
        );
        Ok(record)
    }

    pub fn succeed_with_result(
        &self,
        operation_id: &str,
        message: impl Into<String>,
        result: base::serde_json::Value,
    ) -> GuardResult<OperationRecord> {
        let record = self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Succeeded;
            record.progress_percent = 100;
            record.stage = "ready".to_string();
            record.message = message.into();
            record.result = Some(result);
            record.updated_at_ms = now_ms();
            Ok(())
        })?;
        base::log::info!(
            "operation completed: action=operation, stage=terminal, outcome=succeeded, operation_id={}, kind={}, elapsed_ms={}",
            record.operation_id,
            record.kind,
            record.updated_at_ms.saturating_sub(record.started_at_ms)
        );
        Ok(record)
    }

    pub fn fail(&self, operation_id: &str, error: GuardError) -> GuardResult<OperationRecord> {
        let record = self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Failed;
            record.stage = "failed".to_string();
            record.error = Some(error);
            record.updated_at_ms = now_ms();
            Ok(())
        })?;
        base::log::warn!(
            "operation completed: action=operation, stage=terminal, outcome=failed, operation_id={}, kind={}, elapsed_ms={}",
            record.operation_id,
            record.kind,
            record.updated_at_ms.saturating_sub(record.started_at_ms)
        );
        Ok(record)
    }

    pub fn cancel(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        let mut records = self.records.lock();
        let record = records
            .get_mut(operation_id)
            .ok_or_else(|| GuardError::NotFound(format!("operation {operation_id}")))?;
        if record.status == OperationStatus::Cancelled {
            base::log::debug!(
                "operation cancel reused: action=operation, stage=terminal, outcome=duplicate, operation_id={}, kind={}",
                record.operation_id,
                record.kind
            );
            return Ok(record.clone());
        }
        if record.status.is_terminal() {
            return Err(GuardError::Conflict(format!(
                "operation {operation_id} is terminal"
            )));
        }
        record.status = OperationStatus::Cancelled;
        record.stage = "cancelled".to_string();
        record.message = "operation cancelled".to_string();
        record.updated_at_ms = now_ms();
        let record = record.clone();
        drop(records);
        base::log::info!(
            "operation completed: action=operation, stage=terminal, outcome=cancelled, operation_id={}, kind={}, elapsed_ms={}",
            record.operation_id,
            record.kind,
            record.updated_at_ms.saturating_sub(record.started_at_ms)
        );
        Ok(record)
    }

    pub fn get(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        self.records
            .lock()
            .get(operation_id)
            .cloned()
            .ok_or_else(|| GuardError::NotFound(format!("operation {operation_id}")))
    }

    pub fn list(&self) -> Vec<OperationRecord> {
        let mut records = self.records.lock();
        prune_records(&mut records, now_ms());
        let mut result = records.values().cloned().collect::<Vec<_>>();
        result.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        result
    }

    fn update(
        &self,
        operation_id: &str,
        update: impl FnOnce(&mut OperationRecord) -> GuardResult<()>,
    ) -> GuardResult<OperationRecord> {
        let mut records = self.records.lock();
        let record = records
            .get_mut(operation_id)
            .ok_or_else(|| GuardError::NotFound(format!("operation {operation_id}")))?;
        update(record)?;
        Ok(record.clone())
    }
}

fn prune_records(records: &mut HashMap<String, OperationRecord>, now_ms: i64) {
    records.retain(|_, record| {
        !record.status.is_terminal()
            || now_ms.saturating_sub(record.updated_at_ms) <= TERMINAL_RETENTION_MS
    });
    let excess = records.len().saturating_sub(MAX_OPERATION_RECORDS);
    if excess == 0 {
        return;
    }

    let mut terminal = records
        .values()
        .filter(|record| record.status.is_terminal())
        .map(|record| (record.updated_at_ms, record.operation_id.clone()))
        .collect::<Vec<_>>();
    terminal.sort_unstable();
    for (_, operation_id) in terminal.into_iter().take(excess) {
        records.remove(&operation_id);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> OperationRequest {
        OperationRequest {
            operation_id: "preview-1".to_string(),
            kind: "stream.start".to_string(),
            requested_by: "operator".to_string(),
            caller_role: Role::Operator,
            required_role: Role::Operator,
            dangerous: false,
            confirmation: None,
        }
    }

    #[test]
    fn repeated_operation_request_is_idempotent_for_same_owner_and_kind() {
        let service = OperationService::default();
        let first = service.start(request()).expect("first operation");
        let second = service.start(request()).expect("idempotent operation");
        assert_eq!(first.operation_id, second.operation_id);
    }

    #[test]
    fn start_once_reports_whether_caller_owns_background_execution() {
        let service = OperationService::default();
        let (_, first_created) = service.start_once(request()).expect("first operation");
        let (_, second_created) = service.start_once(request()).expect("existing operation");
        assert!(first_created);
        assert!(!second_created);
    }

    #[test]
    fn pruning_removes_only_expired_terminal_operations() {
        let service = OperationService::default();
        let mut terminal = service.start(request()).expect("terminal operation");
        terminal.status = OperationStatus::Succeeded;
        terminal.updated_at_ms = 1;
        let mut running_request = request();
        running_request.operation_id = "preview-running".to_string();
        let mut running = service.start(running_request).expect("running operation");
        running.status = OperationStatus::Running;
        running.updated_at_ms = 1;

        let mut records = HashMap::from([
            (terminal.operation_id.clone(), terminal),
            (running.operation_id.clone(), running),
        ]);
        prune_records(&mut records, TERMINAL_RETENTION_MS + 2);
        assert!(!records.contains_key("preview-1"));
        assert!(records.contains_key("preview-running"));
    }

    #[test]
    fn media_budget_and_result_are_queryable() {
        let service = OperationService::default();
        service.start(request()).expect("operation");
        service
            .configure_media("preview-1", "waiting_device_response", 8_000, 15_000)
            .expect("media policy");
        service
            .succeed_with_result(
                "preview-1",
                "stream ready",
                base::serde_json::json!({ "stream_id": "stream-1" }),
            )
            .expect("result");
        let record = service.get("preview-1").expect("query operation");
        assert_eq!(record.status, OperationStatus::Succeeded);
        assert_eq!(record.checkpoint_ms, 8_000);
        assert_eq!(record.hard_timeout_ms, 15_000);
        assert_eq!(record.result.expect("result")["stream_id"], "stream-1");
    }
}
