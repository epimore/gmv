use std::collections::HashMap;

use crate::core::{GuardError, GuardResult, LeaseState, RouteState};
use crate::route::reconcile::{ReconcileReport, RecoveryIssue};
use crate::route::snapshot::ResourceSnapshot;
use crate::store::InMemoryGuardStore;
use crate::store::model::{LeaseRecord, RouteRecord};

#[derive(Debug, Clone)]
pub struct RouteService {
    store: InMemoryGuardStore,
}

impl RouteService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn create_allocated(&self, route: RouteRecord) -> GuardResult<()> {
        if self.store.routes().iter().any(|existing| {
            existing.resource_id == route.resource_id
                && !matches!(existing.state, RouteState::Closed | RouteState::Orphaned)
        }) {
            return Err(GuardError::Conflict(format!(
                "resource {} already has active route",
                route.resource_id
            )));
        }
        self.store.upsert_route(route);
        Ok(())
    }

    pub fn apply_snapshot(&self, snapshot: ResourceSnapshot) -> GuardResult<ReconcileReport> {
        let mut issues = Vec::new();
        let mut by_resource: HashMap<String, String> = HashMap::new();
        for resource in &snapshot.resources {
            if let Some(previous_route) = by_resource.insert(
                resource.resource_id.clone(),
                resource.route_id.clone().unwrap_or_default(),
            ) {
                issues.push(RecoveryIssue::Conflict {
                    resource_id: resource.resource_id.clone(),
                    left_route_id: previous_route,
                    right_route_id: resource.route_id.clone().unwrap_or_default(),
                });
            }
        }
        let existing_routes = self.store.routes();
        let mut observed_routes = HashMap::new();
        for resource in &snapshot.resources {
            let Some(route_id) = resource
                .route_id
                .as_deref()
                .filter(|route_id| !route_id.is_empty())
            else {
                continue;
            };
            observed_routes.insert(route_id.to_string(), resource.route_state);
            let existing_route = existing_routes
                .iter()
                .find(|route| route.route_id == route_id);
            if existing_route.is_some_and(|route| {
                route.resource_id != resource.resource_id
                    || route.node_id != snapshot.owner.node_id
                    || route.instance_id != snapshot.owner.instance_id
            }) {
                return Err(GuardError::Conflict(format!(
                    "snapshot route {route_id} conflicts with its stored owner"
                )));
            }
            if existing_route.is_some_and(|route| {
                snapshot.generation < route.observed_generation
                    || snapshot.sequence <= route.observed_sequence
            }) {
                continue;
            }
            if let Some(lease_id) = resource
                .lease_id
                .as_deref()
                .filter(|lease_id| !lease_id.is_empty())
            {
                let desired_lease_state = match resource.route_state {
                    RouteState::Allocated => LeaseState::Allocated,
                    RouteState::Running | RouteState::Reconciling => LeaseState::Confirmed,
                    RouteState::Closed => LeaseState::Released,
                    RouteState::Orphaned | RouteState::Conflict => LeaseState::Failed,
                };
                if let Some(mut lease) = self.store.get_lease(lease_id) {
                    if lease.route_id != route_id
                        || lease.resource_id != resource.resource_id
                        || lease.node_id != snapshot.owner.node_id
                        || lease.instance_id != snapshot.owner.instance_id
                    {
                        return Err(GuardError::Conflict(format!(
                            "snapshot lease {lease_id} conflicts with its stored allocation"
                        )));
                    }
                    lease.state = desired_lease_state;
                    if !resource.endpoints.is_empty() {
                        lease.endpoints.clone_from(&resource.endpoints);
                    }
                    self.store.update_lease(lease)?;
                } else if matches!(
                    resource.route_state,
                    RouteState::Allocated | RouteState::Running | RouteState::Reconciling
                ) {
                    self.store.insert_lease(LeaseRecord {
                        lease_id: lease_id.to_string(),
                        route_id: route_id.to_string(),
                        resource_id: resource.resource_id.clone(),
                        stream_type: resource.resource_type.clone(),
                        node_id: snapshot.owner.node_id.clone(),
                        instance_id: snapshot.owner.instance_id.clone(),
                        idempotency_key: String::new(),
                        constraints: HashMap::new(),
                        endpoints: resource.endpoints.clone(),
                        state: desired_lease_state,
                        expires_at_ms: i64::MAX,
                    })?;
                }
            }
            if existing_route.is_some() {
                continue;
            }
            self.store.upsert_route(RouteRecord {
                route_id: route_id.to_string(),
                resource_id: resource.resource_id.clone(),
                node_id: snapshot.owner.node_id.clone(),
                instance_id: snapshot.owner.instance_id.clone(),
                state: resource.route_state,
                desired_generation: snapshot.generation,
                observed_generation: snapshot.generation,
                observed_sequence: snapshot.sequence,
            });
        }
        for mut route in existing_routes {
            if route.node_id != snapshot.owner.node_id {
                continue;
            }
            if route.instance_id != snapshot.owner.instance_id {
                if snapshot.full && snapshot.generation > route.observed_generation {
                    self.orphan_route(route, snapshot.generation, snapshot.sequence, &mut issues)?;
                } else {
                    issues.push(RecoveryIssue::StaleSnapshot {
                        node_id: snapshot.owner.node_id.clone(),
                    });
                }
                continue;
            }
            let observed_state = observed_routes.get(&route.route_id).copied();
            if !snapshot.full && observed_state.is_none() {
                continue;
            }
            if snapshot.generation < route.observed_generation
                || snapshot.sequence <= route.observed_sequence
            {
                issues.push(RecoveryIssue::StaleSnapshot {
                    node_id: snapshot.owner.node_id.clone(),
                });
                continue;
            }
            if let Some(observed_state) = observed_state {
                route.state = observed_state;
                route.observed_generation = snapshot.generation;
                route.observed_sequence = snapshot.sequence;
            } else if snapshot.full && route.state != RouteState::Closed {
                self.orphan_route(route, snapshot.generation, snapshot.sequence, &mut issues)?;
                continue;
            }
            self.store.upsert_route(route);
        }
        Ok(ReconcileReport { issues })
    }

    fn orphan_route(
        &self,
        mut route: RouteRecord,
        generation: u64,
        sequence: u64,
        issues: &mut Vec<RecoveryIssue>,
    ) -> GuardResult<()> {
        let newly_orphaned = route.state != RouteState::Orphaned;
        route.state = RouteState::Orphaned;
        route.observed_generation = generation;
        route.observed_sequence = sequence;
        for mut lease in self.store.leases().into_iter().filter(|lease| {
            lease.route_id == route.route_id
                && matches!(lease.state, LeaseState::Allocated | LeaseState::Confirmed)
        }) {
            lease.state = LeaseState::Failed;
            self.store.update_lease(lease)?;
        }
        if newly_orphaned {
            issues.push(RecoveryIssue::Orphan {
                resource_id: route.resource_id.clone(),
                node_id: route.node_id.clone(),
            });
        }
        self.store.upsert_route(route);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::core::{NodeIdentity, NodeKind};
    use crate::route::SnapshotResource;
    use crate::store::model::LeaseRecord;

    fn route(route_id: &str, resource_id: &str) -> RouteRecord {
        RouteRecord {
            route_id: route_id.to_string(),
            resource_id: resource_id.to_string(),
            node_id: "stream-a".to_string(),
            instance_id: "instance-a".to_string(),
            state: RouteState::Running,
            desired_generation: 1,
            observed_generation: 1,
            observed_sequence: 1,
        }
    }

    fn snapshot(full: bool, sequence: u64, resources: Vec<SnapshotResource>) -> ResourceSnapshot {
        ResourceSnapshot {
            owner: NodeIdentity::new("stream-a", "instance-a", NodeKind::Stream),
            generation: 1,
            sequence,
            full,
            resources,
        }
    }

    #[test]
    fn partial_snapshot_does_not_orphan_unreported_routes() {
        let store = InMemoryGuardStore::default();
        store.upsert_route(route("route-a", "stream-a"));
        store.upsert_route(route("route-b", "stream-b"));

        RouteService::new(store.clone())
            .apply_snapshot(snapshot(
                false,
                2,
                vec![SnapshotResource {
                    resource_id: "stream-a".to_string(),
                    resource_type: "stream".to_string(),
                    route_id: Some("route-a".to_string()),
                    lease_id: Some("lease-a".to_string()),
                    route_state: RouteState::Running,
                    endpoints: Vec::new(),
                }],
            ))
            .unwrap();

        assert_eq!(
            store.get_route("route-b").unwrap().state,
            RouteState::Running
        );
    }

    #[test]
    fn full_snapshot_orphans_missing_route_and_fails_its_active_lease() {
        let store = InMemoryGuardStore::default();
        store.upsert_route(route("route-a", "stream-a"));
        store
            .insert_lease(LeaseRecord {
                lease_id: "lease-a".to_string(),
                route_id: "route-a".to_string(),
                resource_id: "stream-a".to_string(),
                stream_type: "live".to_string(),
                node_id: "stream-a".to_string(),
                instance_id: "instance-a".to_string(),
                idempotency_key: "operation-a".to_string(),
                constraints: HashMap::new(),
                endpoints: Vec::new(),
                state: LeaseState::Confirmed,
                expires_at_ms: i64::MAX,
            })
            .unwrap();

        let report = RouteService::new(store.clone())
            .apply_snapshot(snapshot(true, 2, Vec::new()))
            .unwrap();

        assert_eq!(report.issues.len(), 1);
        assert_eq!(
            store.get_route("route-a").unwrap().state,
            RouteState::Orphaned
        );
        assert_eq!(
            store.get_lease("lease-a").unwrap().state,
            LeaseState::Failed
        );
        RouteService::new(store.clone())
            .create_allocated(route("route-b", "stream-a"))
            .unwrap();
    }

    #[test]
    fn full_snapshot_from_new_instance_orphans_previous_instance_route() {
        let store = InMemoryGuardStore::default();
        store.upsert_route(route("route-a", "stream-a"));
        store
            .insert_lease(LeaseRecord {
                lease_id: "lease-a".to_string(),
                route_id: "route-a".to_string(),
                resource_id: "stream-a".to_string(),
                stream_type: "live".to_string(),
                node_id: "stream-a".to_string(),
                instance_id: "instance-a".to_string(),
                idempotency_key: "operation-a".to_string(),
                constraints: HashMap::new(),
                endpoints: Vec::new(),
                state: LeaseState::Confirmed,
                expires_at_ms: i64::MAX,
            })
            .unwrap();

        RouteService::new(store.clone())
            .apply_snapshot(ResourceSnapshot {
                owner: NodeIdentity::new("stream-a", "instance-b", NodeKind::Stream),
                generation: 2,
                sequence: 1,
                full: true,
                resources: Vec::new(),
            })
            .unwrap();

        assert_eq!(
            store.get_route("route-a").unwrap().state,
            RouteState::Orphaned
        );
        assert_eq!(
            store.get_lease("lease-a").unwrap().state,
            LeaseState::Failed
        );
    }

    #[test]
    fn snapshot_recovers_consistent_route_and_lease_projection() {
        let store = InMemoryGuardStore::default();

        RouteService::new(store.clone())
            .apply_snapshot(snapshot(
                true,
                1,
                vec![SnapshotResource {
                    resource_id: "stream-a".to_string(),
                    resource_type: "stream".to_string(),
                    route_id: Some("route-a".to_string()),
                    lease_id: Some("lease-a".to_string()),
                    route_state: RouteState::Running,
                    endpoints: Vec::new(),
                }],
            ))
            .unwrap();

        let (lease, route) = store
            .resolve_active_allocation("stream-a")
            .unwrap()
            .unwrap();
        assert_eq!(lease.lease_id, "lease-a");
        assert_eq!(lease.state, LeaseState::Confirmed);
        assert_eq!(route.route_id, "route-a");
        assert_eq!(route.state, RouteState::Running);

        let mut lease = store.get_lease("lease-a").unwrap();
        lease.state = LeaseState::Released;
        store.update_lease(lease).unwrap();
        let mut route = store.get_route("route-a").unwrap();
        route.state = RouteState::Closed;
        store.upsert_route(route);

        RouteService::new(store.clone())
            .apply_snapshot(snapshot(
                true,
                2,
                vec![SnapshotResource {
                    resource_id: "stream-a".to_string(),
                    resource_type: "stream".to_string(),
                    route_id: Some("route-a".to_string()),
                    lease_id: Some("lease-a".to_string()),
                    route_state: RouteState::Running,
                    endpoints: Vec::new(),
                }],
            ))
            .unwrap();

        let (lease, route) = store
            .resolve_active_allocation("stream-a")
            .unwrap()
            .unwrap();
        assert_eq!(lease.state, LeaseState::Confirmed);
        assert_eq!(route.state, RouteState::Running);
    }
}
