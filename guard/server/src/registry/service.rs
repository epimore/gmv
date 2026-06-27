use crate::core::{
    ConnectionState, GuardError, GuardResult, HealthState, NodeIdentity, NodeKind, SchedulingState,
};
use crate::registry::health::scheduling_for_health;
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointRecord, HostMetricsRecord, NodeRecord};

#[derive(Debug, Clone)]
pub struct RegisterRequest {
    pub identity: NodeIdentity,
    pub capabilities: Vec<String>,
    pub endpoints: Vec<EndpointRecord>,
    pub capacity: u32,
    pub host_metrics: HostMetricsRecord,
    pub zone: Option<String>,
    pub now_ms: i64,
    pub takeover: bool,
    pub config: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RegistryPolicy {
    pub allow_unknown_nodes: bool,
    pub allowed_nodes: std::collections::HashMap<String, AllowedNode>,
}

impl Default for RegistryPolicy {
    fn default() -> Self {
        Self {
            allow_unknown_nodes: true,
            allowed_nodes: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AllowedNode {
    pub kind: NodeKind,
    pub required_capabilities: Vec<String>,
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
    pub host_metrics: HostMetricsRecord,
    pub business_metrics: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RegistryService {
    store: InMemoryGuardStore,
    policy: RegistryPolicy,
}

impl RegistryService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self::with_policy(store, RegistryPolicy::default())
    }

    pub fn with_policy(store: InMemoryGuardStore, policy: RegistryPolicy) -> Self {
        Self { store, policy }
    }

    pub fn register(&self, request: RegisterRequest) -> GuardResult<RegisterDecision> {
        request.identity.validate()?;
        if request.capacity == 0 {
            return Err(GuardError::InvalidConfig(
                "node capacity must be positive".to_string(),
            ));
        }
        validate_endpoints(&request.endpoints)?;
        self.validate_policy(&request)?;
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
            endpoints: request.endpoints,
            capabilities: request.capabilities,
            capacity: request.capacity,
            pending_leases: 0,
            host_metrics: request.host_metrics,
            business_metrics: std::collections::HashMap::new(),
            config: request.config,
            zone: request.zone,
            last_seen_at_ms: request.now_ms,
            generation: 1,
            sequence: 0,
        };
        self.store.upsert_node(record);
        Ok(decision)
    }

    fn validate_policy(&self, request: &RegisterRequest) -> GuardResult<()> {
        let Some(allowed) = self.policy.allowed_nodes.get(&request.identity.node_id) else {
            if self.policy.allow_unknown_nodes {
                return Ok(());
            }
            return Err(GuardError::InvalidIdentity(format!(
                "node {} is not allowed",
                request.identity.node_id
            )));
        };
        if allowed.kind != request.identity.kind {
            return Err(GuardError::InvalidIdentity(format!(
                "node {} kind mismatch",
                request.identity.node_id
            )));
        }
        for capability in &allowed.required_capabilities {
            if !request.capabilities.iter().any(|value| value == capability) {
                return Err(GuardError::InvalidConfig(format!(
                    "node {} missing required capability {}",
                    request.identity.node_id, capability
                )));
            }
        }
        Ok(())
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
        node.host_metrics = report.host_metrics;
        node.business_metrics = report.business_metrics;
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

fn validate_endpoints(endpoints: &[EndpointRecord]) -> GuardResult<()> {
    for endpoint in endpoints {
        if endpoint.port == 0 {
            return Err(GuardError::InvalidConfig(format!(
                "node endpoint {} port must be positive",
                endpoint.name
            )));
        }
    }
    Ok(())
}
