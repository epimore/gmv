use crate::core::{GuardError, GuardResult, NodeIdentity};
use crate::gateway::explain::AllocationExplain;
use crate::gateway::filter::eligible;
use crate::gateway::score::ScoreBreakdown;
use crate::store::InMemoryGuardStore;

#[derive(Debug, Clone)]
pub struct AllocationRequest {
    pub request_id: String,
    pub capability: String,
    pub zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AllocationResult {
    pub owner: NodeIdentity,
    pub explain: AllocationExplain,
}

#[derive(Debug, Clone)]
pub struct AllocationService {
    store: InMemoryGuardStore,
}

impl AllocationService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn allocate(&self, request: AllocationRequest) -> GuardResult<AllocationResult> {
        if request.request_id.is_empty() {
            return Err(GuardError::InvalidConfig(
                "allocation request_id is required".to_string(),
            ));
        }
        let candidates = self
            .store
            .nodes()
            .into_iter()
            .filter(|node| eligible(node, &request.capability, request.zone.as_deref()))
            .collect::<Vec<_>>();
        let max_remaining = candidates
            .iter()
            .map(|node| node.capacity.saturating_sub(node.pending_leases))
            .max()
            .unwrap_or(0);
        let mut scores = candidates
            .into_iter()
            .map(|node| {
                let remaining = node.capacity.saturating_sub(node.pending_leases);
                let capacity_score = if max_remaining == 0 {
                    0.0
                } else {
                    remaining as f64 / max_remaining as f64
                };
                let stability_score = 1.0 / (1.0 + node.generation as f64);
                let score = ScoreBreakdown {
                    node_id: node.identity.node_id.clone(),
                    capacity_score,
                    stability_score,
                    total: capacity_score * 0.8 + stability_score * 0.2,
                };
                (score, node)
            })
            .collect::<Vec<_>>();
        scores.sort_by(|(left_score, left_node), (right_score, right_node)| {
            right_score
                .total
                .partial_cmp(&left_score.total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left_node.identity.node_id.cmp(&right_node.identity.node_id))
        });
        let Some((_, selected)) = scores.first() else {
            return Err(GuardError::NotFound("no eligible node".to_string()));
        };
        let score_list = scores
            .iter()
            .map(|(score, _)| score.clone())
            .collect::<Vec<ScoreBreakdown>>();
        Ok(AllocationResult {
            owner: selected.identity.clone(),
            explain: AllocationExplain {
                selected_node_id: selected.identity.node_id.clone(),
                scores: score_list,
            },
        })
    }
}
