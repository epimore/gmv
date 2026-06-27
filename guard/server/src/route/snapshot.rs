use crate::core::NodeIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub owner: NodeIdentity,
    pub generation: u64,
    pub sequence: u64,
    pub resources: Vec<SnapshotResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotResource {
    pub resource_id: String,
    pub route_id: Option<String>,
}
