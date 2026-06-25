use std::collections::HashMap;

use crate::core::{GuardError, GuardResult, RouteState};
use crate::route::reconcile::{ReconcileReport, RecoveryIssue};
use crate::route::snapshot::ResourceSnapshot;
use crate::store::InMemoryGuardStore;
use crate::store::model::RouteRecord;

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
            existing.resource_id == route.resource_id && existing.state != RouteState::Closed
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
        for mut route in self.store.routes() {
            if route.node_id != snapshot.owner.node_id {
                continue;
            }
            if route.instance_id != snapshot.owner.instance_id
                || snapshot.generation < route.observed_generation
                || snapshot.sequence <= route.observed_sequence
            {
                issues.push(RecoveryIssue::StaleSnapshot {
                    node_id: snapshot.owner.node_id.clone(),
                });
                continue;
            }
            let observed = snapshot
                .resources
                .iter()
                .any(|resource| resource.route_id.as_deref() == Some(route.route_id.as_str()));
            if observed {
                route.state = RouteState::Running;
                route.observed_generation = snapshot.generation;
                route.observed_sequence = snapshot.sequence;
            } else if route.state != RouteState::Closed {
                route.state = RouteState::Orphaned;
                issues.push(RecoveryIssue::Orphan {
                    resource_id: route.resource_id.clone(),
                    node_id: route.node_id.clone(),
                });
            }
            self.store.upsert_route(route);
        }
        Ok(ReconcileReport { issues })
    }
}
