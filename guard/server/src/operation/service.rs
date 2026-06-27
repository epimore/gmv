use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::auth::Role;
use crate::core::{GuardError, GuardResult};
use crate::operation::state::{OperationRecord, OperationStatus};

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

        let record = OperationRecord {
            operation_id: request.operation_id,
            kind: request.kind,
            requested_by: request.requested_by,
            required_role: request.required_role,
            status: OperationStatus::Accepted,
            progress_percent: 0,
            message: String::new(),
            error: None,
        };
        let mut records = self.records.lock();
        if records.contains_key(&record.operation_id) {
            return Err(GuardError::Conflict(format!(
                "operation {} already exists",
                record.operation_id
            )));
        }
        records.insert(record.operation_id.clone(), record.clone());
        Ok(record)
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
            Ok(())
        })
    }

    pub fn succeed(
        &self,
        operation_id: &str,
        message: impl Into<String>,
    ) -> GuardResult<OperationRecord> {
        self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Succeeded;
            record.progress_percent = 100;
            record.message = message.into();
            Ok(())
        })
    }

    pub fn fail(&self, operation_id: &str, error: GuardError) -> GuardResult<OperationRecord> {
        self.update(operation_id, |record| {
            if record.status.is_terminal() {
                return Err(GuardError::Conflict(format!(
                    "operation {operation_id} is terminal"
                )));
            }
            record.status = OperationStatus::Failed;
            record.error = Some(error);
            Ok(())
        })
    }

    pub fn get(&self, operation_id: &str) -> GuardResult<OperationRecord> {
        self.records
            .lock()
            .get(operation_id)
            .cloned()
            .ok_or_else(|| GuardError::NotFound(format!("operation {operation_id}")))
    }

    pub fn list(&self) -> Vec<OperationRecord> {
        let mut records = self.records.lock().values().cloned().collect::<Vec<_>>();
        records.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        records
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
