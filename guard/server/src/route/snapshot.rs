use crate::core::{NodeIdentity, RouteState};
use crate::store::model::EndpointRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub owner: NodeIdentity,
    pub generation: u64,
    pub sequence: u64,
    pub full: bool,
    pub resources: Vec<SnapshotResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotResource {
    pub resource_id: String,
    pub resource_type: String,
    pub route_id: Option<String>,
    pub lease_id: Option<String>,
    pub route_state: RouteState,
    pub endpoints: Vec<EndpointRecord>,
}
