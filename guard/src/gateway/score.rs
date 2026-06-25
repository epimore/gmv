use crate::store::model::NodeRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreBreakdown {
    pub node_id: String,
    pub capacity_score: f64,
    pub stability_score: f64,
    pub total: f64,
}

pub fn score(node: &NodeRecord) -> ScoreBreakdown {
    let remaining = node.capacity.saturating_sub(node.pending_leases) as f64;
    let capacity_score = if node.capacity == 0 {
        0.0
    } else {
        remaining / node.capacity as f64
    };
    let stability_score = 1.0 / (1.0 + node.generation as f64);
    ScoreBreakdown {
        node_id: node.identity.node_id.clone(),
        capacity_score,
        stability_score,
        total: capacity_score * 0.8 + stability_score * 0.2,
    }
}
