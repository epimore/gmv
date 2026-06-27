pub mod backup;
pub mod migration;
pub mod model;
pub mod mysql;
pub mod persistent;
pub mod retention;
pub mod sqlite;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::{GuardError, GuardResult};
use model::{EventRecord, LeaseRecord, NodeRecord, OutboxRecord, OutboxState, RouteRecord};

#[derive(Debug, Clone, Default)]
pub struct InMemoryGuardStore {
    inner: Arc<RwLock<StoreInner>>,
}

#[derive(Debug, Default)]
struct StoreInner {
    nodes: HashMap<String, NodeRecord>,
    leases: HashMap<String, LeaseRecord>,
    routes: HashMap<String, RouteRecord>,
    events: HashMap<String, EventRecord>,
    idempotency_keys: HashSet<String>,
    outbox: HashMap<String, OutboxRecord>,
    command_ids: HashMap<String, i64>,
}

impl InMemoryGuardStore {
    pub fn upsert_node(&self, node: NodeRecord) {
        self.inner
            .write()
            .nodes
            .insert(node.identity.node_id.clone(), node);
    }

    pub fn get_node(&self, node_id: &str) -> Option<NodeRecord> {
        self.inner.read().nodes.get(node_id).cloned()
    }

    pub fn nodes(&self) -> Vec<NodeRecord> {
        self.inner.read().nodes.values().cloned().collect()
    }

    pub fn insert_lease(&self, lease: LeaseRecord) -> GuardResult<()> {
        let mut inner = self.inner.write();
        if inner.leases.contains_key(&lease.lease_id) {
            return Err(GuardError::Conflict(format!(
                "lease {} already exists",
                lease.lease_id
            )));
        }
        if !lease.idempotency_key.is_empty()
            && !inner.idempotency_keys.insert(lease.idempotency_key.clone())
        {
            return Err(GuardError::Conflict(format!(
                "idempotency key {} already exists",
                lease.idempotency_key
            )));
        }
        inner.leases.insert(lease.lease_id.clone(), lease);
        Ok(())
    }

    pub fn update_lease(&self, lease: LeaseRecord) -> GuardResult<()> {
        let mut inner = self.inner.write();
        if !inner.leases.contains_key(&lease.lease_id) {
            return Err(GuardError::NotFound(format!(
                "lease {} not found",
                lease.lease_id
            )));
        }
        inner.leases.insert(lease.lease_id.clone(), lease);
        Ok(())
    }

    pub fn get_lease(&self, lease_id: &str) -> Option<LeaseRecord> {
        self.inner.read().leases.get(lease_id).cloned()
    }

    pub fn leases(&self) -> Vec<LeaseRecord> {
        self.inner.read().leases.values().cloned().collect()
    }

    pub fn upsert_route(&self, route: RouteRecord) {
        self.inner
            .write()
            .routes
            .insert(route.route_id.clone(), route);
    }

    pub fn get_route(&self, route_id: &str) -> Option<RouteRecord> {
        self.inner.read().routes.get(route_id).cloned()
    }

    pub fn routes(&self) -> Vec<RouteRecord> {
        self.inner.read().routes.values().cloned().collect()
    }

    pub fn insert_event_once(&self, event: EventRecord) -> GuardResult<bool> {
        let mut inner = self.inner.write();
        if inner.events.contains_key(&event.event_id) {
            return Ok(false);
        }
        inner.events.insert(event.event_id.clone(), event);
        Ok(true)
    }

    pub fn insert_event_with_outbox(
        &self,
        event: EventRecord,
        records: Vec<OutboxRecord>,
    ) -> GuardResult<bool> {
        let mut inner = self.inner.write();
        if inner.events.contains_key(&event.event_id) {
            return Ok(false);
        }
        if records.iter().any(|record| {
            record.event_id != event.event_id
                || record.outbox_id.is_empty()
                || record.destination.is_empty()
                || inner.outbox.contains_key(&record.outbox_id)
        }) {
            return Err(GuardError::Conflict(
                "invalid or duplicate outbox record".to_string(),
            ));
        }
        inner.events.insert(event.event_id.clone(), event);
        for record in records {
            inner.outbox.insert(record.outbox_id.clone(), record);
        }
        Ok(true)
    }

    pub fn get_outbox(&self, outbox_id: &str) -> Option<OutboxRecord> {
        self.inner.read().outbox.get(outbox_id).cloned()
    }

    pub fn due_outbox(&self, now_ms: i64, limit: usize) -> Vec<OutboxRecord> {
        let mut records = self
            .inner
            .read()
            .outbox
            .values()
            .filter(|record| {
                matches!(record.state, OutboxState::Pending | OutboxState::RetryWait)
                    && record.next_attempt_at_ms <= now_ms
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.next_attempt_at_ms
                .cmp(&right.next_attempt_at_ms)
                .then_with(|| left.outbox_id.cmp(&right.outbox_id))
        });
        records.truncate(limit);
        records
    }

    pub fn update_outbox(&self, record: OutboxRecord) -> GuardResult<()> {
        let mut inner = self.inner.write();
        if !inner.outbox.contains_key(&record.outbox_id) {
            return Err(GuardError::NotFound(format!(
                "outbox {} not found",
                record.outbox_id
            )));
        }
        inner.outbox.insert(record.outbox_id.clone(), record);
        Ok(())
    }

    pub fn outbox_records(&self, limit: usize) -> Vec<OutboxRecord> {
        let mut records = self
            .inner
            .read()
            .outbox
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| left.outbox_id.cmp(&right.outbox_id))
        });
        records.truncate(limit);
        records
    }

    pub fn claim_command(&self, command_id: &str, expires_at_ms: i64, now_ms: i64) -> bool {
        let mut inner = self.inner.write();
        inner.command_ids.retain(|_, expires| *expires >= now_ms);
        if inner.command_ids.contains_key(command_id) {
            return false;
        }
        inner
            .command_ids
            .insert(command_id.to_string(), expires_at_ms);
        true
    }

    pub fn recover_stale_sending(&self, stale_before_ms: i64, now_ms: i64) -> usize {
        let mut inner = self.inner.write();
        let mut recovered = 0;
        for record in inner.outbox.values_mut() {
            if record.state == OutboxState::Sending && record.updated_at_ms <= stale_before_ms {
                record.state = OutboxState::RetryWait;
                record.next_attempt_at_ms = now_ms;
                record.last_error = Some("delivery interrupted before completion".to_string());
                record.updated_at_ms = now_ms;
                recovered += 1;
            }
        }
        recovered
    }

    pub fn retry_dead_outbox(&self, outbox_id: &str, now_ms: i64) -> GuardResult<OutboxRecord> {
        let mut inner = self.inner.write();
        let record = inner
            .outbox
            .get_mut(outbox_id)
            .ok_or_else(|| GuardError::NotFound(format!("outbox {outbox_id}")))?;
        if record.state != OutboxState::Dead {
            return Err(GuardError::Conflict(format!(
                "outbox {outbox_id} is not dead"
            )));
        }
        record.state = OutboxState::Pending;
        record.attempts = 0;
        record.next_attempt_at_ms = now_ms;
        record.last_error = None;
        record.updated_at_ms = now_ms;
        Ok(record.clone())
    }

    pub fn events_after(&self, after_id: Option<&str>, limit: usize) -> Vec<EventRecord> {
        let mut events = self
            .inner
            .read()
            .events
            .values()
            .filter(|event| after_id.is_none_or(|cursor| event.event_id.as_str() > cursor))
            .cloned()
            .collect::<Vec<_>>();
        events.sort_by(|left, right| left.event_id.cmp(&right.event_id));
        events.truncate(limit);
        events
    }
}

#[derive(Debug, Clone)]
pub enum GuardStore {
    Memory(InMemoryGuardStore),
    Mysql(mysql::MysqlStore),
    Sqlite(sqlite::SqliteStore),
}
