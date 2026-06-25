use crate::core::{GuardError, GuardResult, LeaseState, NodeIdentity};
use crate::store::InMemoryGuardStore;
use crate::store::model::LeaseRecord;

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub lease_id: String,
    pub route_id: String,
    pub resource_id: String,
    pub idempotency_key: String,
    pub owner: NodeIdentity,
    pub now_ms: i64,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone)]
pub struct LeaseService {
    store: InMemoryGuardStore,
}

impl LeaseService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn allocate(&self, request: LeaseRequest) -> GuardResult<LeaseRecord> {
        request.owner.validate()?;
        let lease = LeaseRecord {
            lease_id: request.lease_id,
            route_id: request.route_id,
            resource_id: request.resource_id,
            node_id: request.owner.node_id,
            instance_id: request.owner.instance_id,
            idempotency_key: request.idempotency_key,
            state: LeaseState::Allocated,
            expires_at_ms: request.now_ms + request.ttl_ms as i64,
        };
        self.store.insert_lease(lease.clone())?;
        Ok(lease)
    }

    pub fn confirm(&self, lease_id: &str, instance_id: &str) -> GuardResult<LeaseRecord> {
        self.transition(lease_id, instance_id, LeaseState::Confirmed)
    }
    pub fn fail(&self, lease_id: &str, instance_id: &str) -> GuardResult<LeaseRecord> {
        self.transition(lease_id, instance_id, LeaseState::Failed)
    }
    pub fn release(&self, lease_id: &str, instance_id: &str) -> GuardResult<LeaseRecord> {
        self.transition(lease_id, instance_id, LeaseState::Released)
    }

    pub fn expire_due(&self, now_ms: i64) -> Vec<String> {
        let mut expired = Vec::new();
        for mut lease in self.store.leases() {
            if lease.state == LeaseState::Allocated && now_ms >= lease.expires_at_ms {
                lease.state = LeaseState::Expired;
                expired.push(lease.lease_id.clone());
                let _ = self.store.update_lease(lease);
            }
        }
        expired
    }

    fn transition(
        &self,
        lease_id: &str,
        instance_id: &str,
        state: LeaseState,
    ) -> GuardResult<LeaseRecord> {
        let mut lease = self
            .store
            .get_lease(lease_id)
            .ok_or_else(|| GuardError::NotFound(format!("lease {lease_id}")))?;
        if lease.instance_id != instance_id {
            return Err(GuardError::StaleInstance(format!(
                "lease {lease_id} belongs to {} not {instance_id}",
                lease.instance_id
            )));
        }
        if matches!(
            lease.state,
            LeaseState::Released | LeaseState::Failed | LeaseState::Expired
        ) {
            return Err(GuardError::Conflict(format!(
                "lease {lease_id} is terminal: {:?}",
                lease.state
            )));
        }
        lease.state = state;
        self.store.update_lease(lease.clone())?;
        Ok(lease)
    }
}
