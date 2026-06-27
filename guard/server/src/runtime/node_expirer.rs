use std::time::Duration;

use crate::registry::RegistryService;

pub fn spawn(registry: RegistryService, timeout_ms: u64) -> base::tokio::task::JoinHandle<()> {
    base::tokio::spawn(async move {
        let interval_ms = (timeout_ms / 2).clamp(500, 5_000);
        let mut interval = base::tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            interval.tick().await;
            let expired = registry.expire_stale(now_ms(), timeout_ms);
            if !expired.is_empty() {
                base::log::warn!("Guard marked stale nodes offline: {}", expired.join(","));
            }
        }
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ConnectionState, HealthState, NodeIdentity, NodeKind, SchedulingState};
    use crate::registry::{RegisterRequest, RegistryService};
    use crate::store::InMemoryGuardStore;

    #[test]
    fn background_task_marks_stale_nodes_offline() {
        base::tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let store = InMemoryGuardStore::default();
                let registry = RegistryService::new(store.clone());
                registry
                    .register(RegisterRequest {
                        identity: NodeIdentity::new("stream-expire", "inst-1", NodeKind::Stream),
                        capabilities: vec!["live".to_string()],
                        endpoints: vec![],
                        capacity: 1,
                        host_metrics: Default::default(),
                        zone: None,
                        now_ms: now_ms() - 1_000,
                        takeover: false,
                        config: Default::default(),
                    })
                    .unwrap();
                let handle = spawn(registry, 10);
                base::tokio::time::sleep(Duration::from_millis(650)).await;
                handle.abort();
                let node = store.get_node("stream-expire").unwrap();
                assert_eq!(node.connection, ConnectionState::Disconnected);
                assert_eq!(node.health, HealthState::Offline);
                assert_eq!(node.scheduling, SchedulingState::Disabled);
            });
    }
}
