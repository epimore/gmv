use crate::store::model::NodeRecord;

use super::filter::PENDING_LEASE_LIMIT;

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreBreakdown {
    pub node_id: String,
    pub queue_score: f64,
    pub stability_score: f64,
    pub total: f64,
}

pub fn score(node: &NodeRecord) -> ScoreBreakdown {
    let remaining = PENDING_LEASE_LIMIT.saturating_sub(node.pending_leases) as f64;
    let queue_score = remaining / PENDING_LEASE_LIMIT as f64;
    let stability_score = 1.0 / (1.0 + node.generation as f64);
    ScoreBreakdown {
        node_id: node.identity.node_id.clone(),
        queue_score,
        stability_score,
        total: queue_score * 0.8 + stability_score * 0.2,
    }
}
