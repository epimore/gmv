use crate::core::{HealthState, SchedulingState};
use crate::store::model::NodeRecord;

pub const PENDING_LEASE_LIMIT: u32 = 100;

pub fn eligible(node: &NodeRecord, capability: &str, zone: Option<&str>) -> bool {
    node.health == HealthState::Ready
        && node.scheduling == SchedulingState::Enabled
        && node.pending_leases < PENDING_LEASE_LIMIT
        && node.capabilities.iter().any(|item| item == capability)
        && zone.is_none_or(|expected| node.zone.as_deref() == Some(expected))
}
