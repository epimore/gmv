use crate::auth::Role;
use crate::core::GuardError;
use base::serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    pub operation_id: String,
    pub kind: String,
    pub requested_by: String,
    pub required_role: Role,
    pub status: OperationStatus,
    pub progress_percent: u8,
    pub stage: String,
    pub message: String,
    pub error: Option<GuardError>,
    pub result: Option<Value>,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub checkpoint_ms: u64,
    pub hard_timeout_ms: u64,
}
