#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupJob {
    pub job_id: String,
    pub target_path: String,
}
