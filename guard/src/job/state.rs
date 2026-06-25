use crate::core::GuardError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemJobType {
    Backup,
    Restore,
    Migrate,
    Reconcile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemJobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

impl SystemJobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemJobRecord {
    pub job_id: String,
    pub job_type: SystemJobType,
    pub status: SystemJobStatus,
    pub progress_percent: u8,
    pub message: String,
    pub error: Option<GuardError>,
}
