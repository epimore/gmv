#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub event_retention_days: u32,
    pub audit_retention_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            event_retention_days: 30,
            audit_retention_days: 180,
        }
    }
}
