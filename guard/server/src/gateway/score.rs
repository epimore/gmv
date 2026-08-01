#[derive(Debug, Clone, PartialEq)]
pub struct ScoreBreakdown {
    pub node_id: String,
    pub host_id: String,
    pub active_allocated: u32,
    pub active_confirmed: u32,
    pub host_active: u32,
    pub tcp_passive_broadcasts: u32,
    pub load_cost: f64,
    pub tie_breaker: u64,
    pub eligible: bool,
    pub reason: String,
    pub total: f64,
}
