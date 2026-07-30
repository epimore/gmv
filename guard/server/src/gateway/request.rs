use std::collections::HashMap;

use crate::core::{GuardError, GuardResult, LeaseState, NodeIdentity};
use crate::gateway::explain::AllocationExplain;
use crate::gateway::filter::eligible;
use crate::gateway::score::ScoreBreakdown;
use crate::store::InMemoryGuardStore;
use crate::store::model::EndpointModeRecord;
use crate::store::model::{LeaseRecord, NodeRecord};

#[derive(Debug, Clone)]
pub struct AllocationRequest {
    pub request_id: String,
    pub resource_id: String,
    pub capability: String,
    pub zone: Option<String>,
    pub constraints: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AllocationResult {
    pub owner: NodeIdentity,
    pub explain: AllocationExplain,
}

#[derive(Debug, Clone)]
pub struct AllocationService {
    store: InMemoryGuardStore,
}

impl AllocationService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub fn allocate(&self, request: AllocationRequest) -> GuardResult<AllocationResult> {
        if request.request_id.is_empty() {
            return Err(GuardError::InvalidConfig(
                "allocation request_id is required".to_string(),
            ));
        }
        let leases = self.store.leases();
        let requires_tcp_passive_isolation =
            requires_tcp_passive_isolation(&request.capability, &request.constraints);
        let nodes = self.store.nodes();
        let host_by_node = nodes
            .iter()
            .map(|node| (node.identity.node_id.clone(), host_id(node)))
            .collect::<HashMap<_, _>>();
        let candidates = nodes
            .into_iter()
            .filter(|node| eligible(node, &request.capability, request.zone.as_deref()))
            .collect::<Vec<_>>();
        let mut scores = candidates
            .into_iter()
            .map(|node| {
                let load = active_load(&leases, &node.identity.node_id);
                let host_id = host_id(&node);
                let host_active = active_host_load(&leases, &host_by_node, &host_id);
                let tcp_passive_busy = requires_tcp_passive_isolation && load.tcp_passive_talks > 0;
                let media_capacity_exhausted = media_capacity_exhausted(&node);
                let eligible = !tcp_passive_busy && !media_capacity_exhausted;
                let reason = if tcp_passive_busy {
                    "tcp_passive_domain_busy".to_string()
                } else if media_capacity_exhausted {
                    "media_port_pool_exhausted".to_string()
                } else {
                    "eligible".to_string()
                };
                let load_cost = load.active_confirmed as f64
                    + load.active_allocated as f64 * 0.5
                    + host_active as f64 * 0.25;
                let score = ScoreBreakdown {
                    node_id: node.identity.node_id.clone(),
                    host_id,
                    active_allocated: load.active_allocated,
                    active_confirmed: load.active_confirmed,
                    host_active,
                    tcp_passive_talks: load.tcp_passive_talks,
                    load_cost,
                    tie_breaker: stable_tie_breaker(&request.resource_id, &node.identity.node_id),
                    eligible,
                    reason,
                    total: load_cost,
                };
                (score, node)
            })
            .collect::<Vec<_>>();
        scores.sort_by(|(left_score, left_node), (right_score, right_node)| {
            left_score
                .eligible
                .cmp(&right_score.eligible)
                .reverse()
                .then_with(|| {
                    left_score
                        .load_cost
                        .partial_cmp(&right_score.load_cost)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left_score.tie_breaker.cmp(&right_score.tie_breaker))
                .then_with(|| left_node.identity.node_id.cmp(&right_node.identity.node_id))
        });
        let Some((_, selected)) = scores.iter().find(|(score, _)| score.eligible) else {
            return Err(GuardError::NotFound(
                "no eligible node after stream isolation constraints".to_string(),
            ));
        };
        let score_list = scores
            .iter()
            .map(|(score, _)| score.clone())
            .collect::<Vec<ScoreBreakdown>>();
        Ok(AllocationResult {
            owner: selected.identity.clone(),
            explain: AllocationExplain {
                selected_node_id: selected.identity.node_id.clone(),
                scores: score_list,
            },
        })
    }
}

fn media_capacity_exhausted(node: &NodeRecord) -> bool {
    let dynamic_media = node
        .endpoints
        .iter()
        .any(|endpoint| endpoint.name == "rtp" && endpoint.mode == EndpointModeRecord::Multi);
    dynamic_media
        && node
            .business_metrics
            .get("media_ports_free")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(0)
}

#[derive(Default)]
struct ActiveLoad {
    active_allocated: u32,
    active_confirmed: u32,
    tcp_passive_talks: u32,
}

fn active_load(leases: &[LeaseRecord], node_id: &str) -> ActiveLoad {
    let mut load = ActiveLoad::default();
    for lease in leases {
        if lease.node_id != node_id {
            continue;
        }
        match lease.state {
            LeaseState::Allocated => load.active_allocated += 1,
            LeaseState::Confirmed => load.active_confirmed += 1,
            LeaseState::Failed | LeaseState::Released | LeaseState::Expired => continue,
        }
        if requires_tcp_passive_isolation(&lease.stream_type, &lease.constraints) {
            load.tcp_passive_talks += 1;
        }
    }
    load
}

fn active_host_load(
    leases: &[LeaseRecord],
    host_by_node: &HashMap<String, String>,
    host_id: &str,
) -> u32 {
    leases
        .iter()
        .filter(|lease| matches!(lease.state, LeaseState::Allocated | LeaseState::Confirmed))
        .filter(|lease| {
            host_by_node
                .get(&lease.node_id)
                .is_some_and(|host| host == host_id)
        })
        .count() as u32
}

fn host_id(node: &NodeRecord) -> String {
    node.config
        .get("host_id")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| node.endpoints.first().map(|endpoint| endpoint.host.clone()))
        .unwrap_or_else(|| node.identity.node_id.clone())
}

fn requires_tcp_passive_isolation(
    stream_type: &str,
    constraints: &HashMap<String, String>,
) -> bool {
    if stream_type != "talk" {
        return false;
    }
    constraints
        .get("requires_dedicated_media_endpoint")
        .is_some_and(|value| truthy(value))
        || constraints
            .get("transport")
            .is_some_and(|value| normalized(value) == "tcppassive")
}

fn truthy(value: &str) -> bool {
    matches!(normalized(value).as_str(), "1" | "true" | "yes" | "on")
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn stable_tie_breaker(resource_id: &str, node_id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in resource_id
        .as_bytes()
        .iter()
        .chain([b':'].iter())
        .chain(node_id.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
