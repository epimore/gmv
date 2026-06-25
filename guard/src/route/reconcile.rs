#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub issues: Vec<RecoveryIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryIssue {
    Orphan {
        resource_id: String,
        node_id: String,
    },
    Conflict {
        resource_id: String,
        left_route_id: String,
        right_route_id: String,
    },
    StaleSnapshot {
        node_id: String,
    },
}
