pub mod integration;
pub mod migration;
pub mod model;
#[cfg(feature = "db-mysql")]
pub mod mysql;
pub mod persistent;
#[cfg(feature = "db-sqlite")]
pub mod sqlite;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::{GuardError, GuardResult};
use model::{
    EventRecord, LeaseRecord, NodeRecord, OutboxRecord, OutboxState, PlaybackTicketRecord,
    RouteRecord, StreamSessionOwnerRecord,
};

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
    integration_delivery_keys: HashMap<(String, String), i64>,
    playback_tickets: HashMap<String, PlaybackTicketRecord>,
    stream_session_owners: HashMap<String, StreamSessionOwnerRecord>,
    stream_input_owners: HashMap<String, StreamSessionOwnerRecord>,
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

    pub fn upsert_playback_ticket(&self, ticket: PlaybackTicketRecord) {
        self.inner
            .write()
            .playback_tickets
            .insert(ticket.token.clone(), ticket);
    }

    pub fn get_playback_ticket(&self, token: &str) -> Option<PlaybackTicketRecord> {
        self.inner.read().playback_tickets.get(token).cloned()
    }

    pub fn integration_playback_tickets_expiring_before(
        &self,
        now_ms: i64,
        before_ms: i64,
    ) -> Vec<PlaybackTicketRecord> {
        self.inner
            .read()
            .playback_tickets
            .values()
            .filter(|ticket| {
                ticket.username.starts_with("integration:")
                    && ticket.expires_at_ms > now_ms
                    && ticket.expires_at_ms <= before_ms
            })
            .cloned()
            .collect()
    }

    pub fn find_playback_control_ticket(
        &self,
        playback_id: &str,
        stream_id: &str,
    ) -> Option<PlaybackTicketRecord> {
        self.inner
            .read()
            .playback_tickets
            .values()
            .find(|ticket| ticket.playback_id == playback_id && ticket.stream_id == stream_id)
            .cloned()
    }

    pub fn playback_tickets_for_subscription(
        &self,
        stream_id: &str,
        subscription_id: &str,
    ) -> Vec<PlaybackTicketRecord> {
        self.inner
            .read()
            .playback_tickets
            .values()
            .filter(|ticket| {
                ticket.stream_id == stream_id && ticket.subscription_id == subscription_id
            })
            .cloned()
            .collect()
    }

    pub fn has_playback_ticket_for_stream(&self, stream_id: &str, now_ms: i64) -> bool {
        self.inner.read().playback_tickets.values().any(|ticket| {
            ticket.stream_id == stream_id
                && ticket.expires_at_ms > now_ms
                && (!ticket.playback_id.is_empty()
                    || ticket.playback_start_time_sec > 0
                    || ticket.playback_end_time_sec > 0)
        })
    }

    pub fn revoke_playback_token(&self, token: &str) {
        self.inner.write().playback_tickets.remove(token);
    }

    pub fn revoke_playback_tickets_for_stream(&self, stream_id: &str) -> usize {
        let mut inner = self.inner.write();
        let before = inner.playback_tickets.len();
        inner
            .playback_tickets
            .retain(|_, ticket| ticket.stream_id != stream_id);
        before - inner.playback_tickets.len()
    }

    pub fn revoke_playback_tickets_for_output(&self, stream_id: &str, output_id: &str) -> usize {
        let mut inner = self.inner.write();
        let before = inner.playback_tickets.len();
        inner
            .playback_tickets
            .retain(|_, ticket| ticket.stream_id != stream_id || ticket.output_id != output_id);
        before - inner.playback_tickets.len()
    }

    pub fn revoke_playback_tickets_for_subscription(
        &self,
        stream_id: &str,
        subscription_id: &str,
    ) -> usize {
        let mut inner = self.inner.write();
        let before = inner.playback_tickets.len();
        inner.playback_tickets.retain(|_, ticket| {
            ticket.stream_id != stream_id || ticket.subscription_id != subscription_id
        });
        before - inner.playback_tickets.len()
    }

    pub fn upsert_stream_session_owner(&self, owner: StreamSessionOwnerRecord) {
        let mut inner = self.inner.write();
        if !owner.input_key.is_empty() {
            inner
                .stream_input_owners
                .insert(owner.input_key.clone(), owner.clone());
        }
        inner
            .stream_session_owners
            .insert(owner.stream_id.clone(), owner);
    }

    pub fn get_stream_session_owner(&self, stream_id: &str) -> Option<StreamSessionOwnerRecord> {
        self.inner
            .read()
            .stream_session_owners
            .get(stream_id)
            .cloned()
    }

    pub fn get_stream_session_owner_by_input(
        &self,
        input_key: &str,
    ) -> Option<StreamSessionOwnerRecord> {
        self.inner
            .read()
            .stream_input_owners
            .get(input_key)
            .cloned()
    }

    pub fn claim_stream_input_owner(
        &self,
        input_key: &str,
        candidate: StreamSessionOwnerRecord,
    ) -> StreamSessionOwnerRecord {
        self.inner
            .write()
            .stream_input_owners
            .entry(input_key.to_string())
            .or_insert(candidate)
            .clone()
    }

    pub fn replace_inactive_stream_input_owner(
        &self,
        input_key: &str,
        candidate: StreamSessionOwnerRecord,
    ) -> StreamSessionOwnerRecord {
        let mut inner = self.inner.write();
        let owner = inner
            .stream_input_owners
            .entry(input_key.to_string())
            .or_insert_with(|| candidate.clone());
        if owner.stream_id.is_empty() {
            *owner = candidate;
        }
        owner.clone()
    }

    pub fn remove_stream_session_owner(&self, stream_id: &str) {
        let mut inner = self.inner.write();
        if let Some(owner) = inner.stream_session_owners.remove(stream_id)
            && !owner.input_key.is_empty()
            && let Some(current) = inner.stream_input_owners.get_mut(&owner.input_key)
            && current.stream_id == stream_id
        {
            current.stream_id.clear();
        }
    }

    pub fn insert_event_once(&self, event: EventRecord) -> GuardResult<bool> {
        let mut inner = self.inner.write();
        if inner.events.contains_key(&event.event_id) {
            return Ok(false);
        }
        inner.events.insert(event.event_id.clone(), event);
        Ok(true)
    }

    pub fn remove_event(&self, event_id: &str) {
        self.inner.write().events.remove(event_id);
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

    pub fn insert_outbox_records(&self, records: Vec<OutboxRecord>) -> GuardResult<()> {
        let mut inner = self.inner.write();
        for record in &records {
            if record.outbox_id.is_empty()
                || record.destination.is_empty()
                || inner.outbox.contains_key(&record.outbox_id)
            {
                return Err(GuardError::Conflict(
                    "invalid or duplicate outbox record".to_string(),
                ));
            }
        }
        for record in records {
            inner.outbox.insert(record.outbox_id.clone(), record);
        }
        Ok(())
    }

    pub fn insert_mapped_outbox_records(&self, records: Vec<OutboxRecord>) -> GuardResult<()> {
        let mut inner = self.inner.write();
        let now_ms = records
            .iter()
            .map(|record| record.created_at_ms)
            .max()
            .unwrap_or_default();
        inner
            .integration_delivery_keys
            .retain(|_, expires_at_ms| *expires_at_ms > now_ms);
        for record in records {
            if record.outbox_id.is_empty()
                || record.event_id.is_empty()
                || record.mapping_id.is_empty()
                || record.destination.is_empty()
            {
                return Err(GuardError::InvalidConfig(
                    "invalid mapped outbox record".to_string(),
                ));
            }
            let key = (record.event_id.clone(), record.mapping_id.clone());
            let expires_at_ms = record
                .expires_at_ms
                .unwrap_or_else(|| record.created_at_ms.saturating_add(259_200_000));
            if inner
                .integration_delivery_keys
                .insert(key, expires_at_ms)
                .is_none()
            {
                inner.outbox.insert(record.outbox_id.clone(), record);
            }
        }
        Ok(())
    }

    pub fn get_outbox(&self, outbox_id: &str) -> Option<OutboxRecord> {
        self.inner.read().outbox.get(outbox_id).cloned()
    }

    pub fn remove_outbox(&self, outbox_id: &str) -> bool {
        self.inner.write().outbox.remove(outbox_id).is_some()
    }

    pub fn cleanup_dead_outbox(&self, older_than_ms: i64, max_per_integration: usize) -> usize {
        let mut inner = self.inner.write();
        let before = inner.outbox.len();
        inner.outbox.retain(|_, record| {
            record.state != OutboxState::Dead || record.updated_at_ms > older_than_ms
        });
        let mut by_integration = HashMap::<String, Vec<(String, i64)>>::new();
        for record in inner
            .outbox
            .values()
            .filter(|record| record.state == OutboxState::Dead)
        {
            by_integration
                .entry(record.integration_id.clone())
                .or_default()
                .push((record.outbox_id.clone(), record.updated_at_ms));
        }
        for records in by_integration.values_mut() {
            records.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            for (outbox_id, _) in records.iter().skip(max_per_integration) {
                inner.outbox.remove(outbox_id);
            }
        }
        before.saturating_sub(inner.outbox.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    fn playback_ticket(token: &str, playback_id: &str, expires_at_ms: i64) -> PlaybackTicketRecord {
        PlaybackTicketRecord {
            token: token.to_string(),
            stream_id: "stream-a".to_string(),
            playback_id: playback_id.to_string(),
            playback_start_time_sec: 0,
            playback_end_time_sec: 0,
            output_id: String::new(),
            subscription_id: String::new(),
            lease_id: String::new(),
            route_id: String::new(),
            username: String::new(),
            ui_session_token: String::new(),
            required_role: Role::Viewer,
            issued_at_ms: 0,
            expires_at_ms,
            absolute_expires_at_ms: i64::MAX,
            renewal_count: 0,
        }
    }

    #[test]
    fn live_access_ticket_does_not_mark_stream_as_playback() {
        let store = InMemoryGuardStore::default();
        store.upsert_playback_ticket(playback_ticket("live-token", "", 2_000));
        assert!(!store.has_playback_ticket_for_stream("stream-a", 1_000));

        store.upsert_playback_ticket(playback_ticket("playback-token", "playback-a", 2_000));
        assert!(store.has_playback_ticket_for_stream("stream-a", 1_000));
        assert!(!store.has_playback_ticket_for_stream("stream-a", 2_000));
    }
}
