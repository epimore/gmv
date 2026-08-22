use crate::core::{
    ConnectionState, GuardError, GuardResult, HealthState, NodeIdentity, NodeKind, SchedulingState,
};
use crate::registry::health::scheduling_for_health;
use crate::store::InMemoryGuardStore;
use crate::store::model::{EndpointRecord, HostMetricsRecord, NodeRecord};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RegisterRequest {
    pub identity: NodeIdentity,
    pub capabilities: Vec<String>,
    pub endpoints: Vec<EndpointRecord>,
    pub host_metrics: HostMetricsRecord,
    pub zone: Option<String>,
    pub now_ms: i64,
    pub takeover: bool,
    pub config: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct RegistryPolicy {
    pub node_check_enabled: bool,
    pub allowed_nodes: std::collections::HashMap<String, AllowedNode>,
}

#[derive(Debug, Clone)]
pub struct AllowedNode {
    pub kind: NodeKind,
    pub service: String,
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
    register_lock: Arc<parking_lot::Mutex<()>>,
}

impl RegistryService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self::with_policy(store, RegistryPolicy::default())
    }

    pub fn with_policy(store: InMemoryGuardStore, policy: RegistryPolicy) -> Self {
        let service = Self {
            store,
            policy,
            register_lock: Arc::new(parking_lot::Mutex::new(())),
        };
        if service.policy.node_check_enabled {
            service.seed_allowed_nodes();
        }
        service
    }

    fn seed_allowed_nodes(&self) {
        for (node_id, allowed) in &self.policy.allowed_nodes {
            if self.store.get_node(node_id).is_some() {
                continue;
            }
            self.store.upsert_node(NodeRecord {
                identity: NodeIdentity::new(node_id.clone(), "offline", allowed.kind),
                connection: ConnectionState::Disconnected,
                health: HealthState::Offline,
                scheduling: SchedulingState::Disabled,
                endpoints: Vec::new(),
                capabilities: Vec::new(),
                pending_leases: 0,
                host_metrics: HostMetricsRecord::default(),
                business_metrics: std::collections::HashMap::new(),
                config: std::collections::HashMap::from([(
                    "service".to_string(),
                    allowed.service.clone(),
                )]),
                zone: None,
                last_seen_at_ms: 0,
                generation: 0,
                sequence: 0,
            });
        }
    }

    pub fn register(&self, request: RegisterRequest) -> GuardResult<RegisterDecision> {
        let _register_guard = self.register_lock.lock();
        request.identity.validate()?;
        validate_endpoints(&request.endpoints)?;
        self.validate_policy(&request)?;
        if request.identity.kind == NodeKind::Session
            && request
                .config
                .get("protocol")
                .is_some_and(|value| value == "gb28181")
            && request.config.get("domain_id") != Some(&request.identity.node_id)
        {
            return Err(GuardError::InvalidIdentity(
                "GB28181 session node_id must equal domain_id".to_string(),
            ));
        }
        let existing = self.store.get_node(&request.identity.node_id);
        let decision = match existing.as_ref() {
            None => RegisterDecision::Accepted,
            Some(existing)
                if existing.connection == ConnectionState::Disconnected
                    && existing.generation == 0 =>
            {
                RegisterDecision::Accepted
            }
            Some(existing) if existing.identity.instance_id == request.identity.instance_id => {
                RegisterDecision::Reconnected
            }
            Some(existing)
                if existing.connection == ConnectionState::Connected
                    && (!request.takeover || request.identity.kind == NodeKind::Session) =>
            {
                return Err(GuardError::Conflict(format!(
                    "node {} already has active instance {}",
                    existing.identity.node_id, existing.identity.instance_id
                )));
            }
            Some(_) => RegisterDecision::SupersededOldInstance,
        };
        let mut config = request.config;
        if self.policy.node_check_enabled
            && let Some(allowed) = self.policy.allowed_nodes.get(&request.identity.node_id)
        {
            config
                .entry("service".to_string())
                .or_insert_with(|| allowed.service.clone());
        }
        let record = NodeRecord {
            identity: request.identity,
            connection: ConnectionState::Connected,
            health: HealthState::Ready,
            scheduling: SchedulingState::Enabled,
            endpoints: request.endpoints,
            capabilities: request.capabilities,
            pending_leases: 0,
            host_metrics: request.host_metrics,
            business_metrics: std::collections::HashMap::new(),
            config,
            zone: request.zone,
            last_seen_at_ms: request.now_ms,
            generation: existing.map_or(1, |node| node.generation.saturating_add(1)),
            sequence: 0,
        };
        self.store.upsert_node(record);
        Ok(decision)
    }

    fn validate_policy(&self, request: &RegisterRequest) -> GuardResult<()> {
        if !self.policy.node_check_enabled {
            return Ok(());
        }
        let Some(allowed) = self.policy.allowed_nodes.get(&request.identity.node_id) else {
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
            if node.connection == ConnectionState::Connected
                && now_ms.saturating_sub(node.last_seen_at_ms) > timeout_ms as i64
            {
                node.connection = ConnectionState::Disconnected;
                node.health = HealthState::Offline;
                node.scheduling = SchedulingState::Disabled;
                expired.push(node.identity.node_id.clone());
                self.store.upsert_node(node);
            }
        }
        expired
    }

    pub fn disconnect_if_current(&self, identity: &NodeIdentity, generation: u64) -> bool {
        let Some(mut node) = self.store.get_node(&identity.node_id) else {
            return false;
        };
        if node.identity.instance_id != identity.instance_id
            || node.generation != generation
            || node.connection != ConnectionState::Connected
        {
            return false;
        }
        node.connection = ConnectionState::Disconnected;
        node.health = HealthState::Offline;
        node.scheduling = SchedulingState::Disabled;
        self.store.upsert_node(node);
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(instance_id: &str, kind: NodeKind, takeover: bool) -> RegisterRequest {
        RegisterRequest {
            identity: NodeIdentity::new("domain-1", instance_id, kind),
            capabilities: Vec::new(),
            endpoints: Vec::new(),
            host_metrics: HostMetricsRecord::default(),
            zone: None,
            now_ms: 1,
            takeover,
            config: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn connected_session_domain_rejects_takeover() {
        let registry = RegistryService::new(InMemoryGuardStore::default());
        assert_eq!(
            registry
                .register(request("instance-a", NodeKind::Session, false))
                .expect("register session"),
            RegisterDecision::Accepted
        );
        assert!(
            registry
                .register(request("instance-b", NodeKind::Session, true))
                .is_err()
        );
    }
}
