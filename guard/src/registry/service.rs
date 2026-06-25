use crate::core::{
    ConnectionState, GuardError, GuardResult, HealthState, NodeIdentity, SchedulingState,
};
use crate::registry::health::scheduling_for_health;
use crate::store::InMemoryGuardStore;
use crate::store::model::NodeRecord;

#[derive(Debug, Clone)]
pub struct RegisterRequest {
    pub identity: NodeIdentity,
    pub capabilities: Vec<String>,
    pub capacity: u32,
    pub zone: Option<String>,
    pub now_ms: i64,
    pub takeover: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterDecision {
    Accepted,
    Reconnected,
    SupersededOldInstance,
}

#[derive(Debug, Clone)]
pub struct HeartbeatReport {
    pub identity: NodeIdentity,
    pub health: HealthState,
    pub sequence: u64,
    pub now_ms: i64,
}

#[derive(Debug, Clone)]
pub struct RegistryService {
    store: InMemoryGuardStore,
}

impl RegistryService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn register(&self, request: RegisterRequest) -> GuardResult<RegisterDecision> {
        request.identity.validate()?;
        if request.capacity == 0 {
            return Err(GuardError::InvalidConfig(
                "node capacity must be positive".to_string(),
            ));
        }
        let decision = match self.store.get_node(&request.identity.node_id) {
            None => RegisterDecision::Accepted,
            Some(existing) if existing.identity.instance_id == request.identity.instance_id => {
                RegisterDecision::Reconnected
            }
            Some(existing)
                if existing.connection == ConnectionState::Connected && !request.takeover =>
            {
                return Err(GuardError::Conflict(format!(
                    "node {} already has active instance {}",
                    existing.identity.node_id, existing.identity.instance_id
                )));
            }
            Some(_) => RegisterDecision::SupersededOldInstance,
        };
        let record = NodeRecord {
            identity: request.identity,
            connection: ConnectionState::Connected,
            health: HealthState::Ready,
            scheduling: SchedulingState::Enabled,
            capabilities: request.capabilities,
            capacity: request.capacity,
            pending_leases: 0,
            zone: request.zone,
            last_seen_at_ms: request.now_ms,
            generation: 1,
            sequence: 0,
        };
        self.store.upsert_node(record);
        Ok(decision)
    }

    pub fn heartbeat(&self, report: HeartbeatReport) -> GuardResult<()> {
        report.identity.validate()?;
        let mut node = self
            .store
            .get_node(&report.identity.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", report.identity.node_id)))?;
        if node.identity.instance_id != report.identity.instance_id {
            return Err(GuardError::StaleInstance(format!(
                "node {} stale instance {} current {}",
                report.identity.node_id, report.identity.instance_id, node.identity.instance_id
            )));
        }
        if report.sequence <= node.sequence {
            return Err(GuardError::StaleInstance(format!(
                "node {} stale sequence {} <= {}",
                report.identity.node_id, report.sequence, node.sequence
            )));
        }
        node.health = report.health;
        node.scheduling = scheduling_for_health(report.health);
        node.last_seen_at_ms = report.now_ms;
        node.sequence = report.sequence;
        self.store.upsert_node(node);
        Ok(())
    }

    pub fn expire_stale(&self, now_ms: i64, timeout_ms: u64) -> Vec<String> {
        let mut expired = Vec::new();
        for mut node in self.store.nodes() {
            if now_ms.saturating_sub(node.last_seen_at_ms) > timeout_ms as i64 {
                node.connection = ConnectionState::Disconnected;
                node.health = HealthState::Offline;
                node.scheduling = SchedulingState::Disabled;
                expired.push(node.identity.node_id.clone());
                self.store.upsert_node(node);
            }
        }
        expired
    }
}
