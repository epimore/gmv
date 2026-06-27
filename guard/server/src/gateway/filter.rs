use crate::core::{HealthState, SchedulingState};
use crate::store::model::NodeRecord;

pub fn eligible(node: &NodeRecord, capability: &str, zone: Option<&str>) -> bool {
    node.health == HealthState::Ready
        && node.scheduling == SchedulingState::Enabled
        && node.pending_leases < node.capacity
        && node.capabilities.iter().any(|item| item == capability)
        && zone.is_none_or(|expected| node.zone.as_deref() == Some(expected))
}
