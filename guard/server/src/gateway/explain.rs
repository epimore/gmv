use crate::gateway::score::ScoreBreakdown;

#[derive(Debug, Clone, PartialEq)]
pub struct AllocationExplain {
    pub selected_node_id: String,
    pub scores: Vec<ScoreBreakdown>,
}
