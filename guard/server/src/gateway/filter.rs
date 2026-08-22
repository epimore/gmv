use crate::core::{HealthState, SchedulingState};
use crate::store::model::NodeRecord;

pub const PENDING_LEASE_LIMIT: u32 = 100;

pub fn eligible(node: &NodeRecord, capability: &str, zone: Option<&str>) -> bool {
    node.health == HealthState::Ready
        && node.scheduling == SchedulingState::Enabled
        && node.pending_leases < PENDING_LEASE_LIMIT
        && !node.config.get("drain").is_some_and(|value| truthy(value))
        && node.capabilities.iter().any(|item| item == capability)
        && zone.is_none_or(|expected| node.zone.as_deref() == Some(expected))
}

fn truthy(value: &str) -> bool {
    matches!(
        value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect::<String>()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}
