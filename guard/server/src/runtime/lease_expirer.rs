use std::time::Duration;

use base::exception::GlobalResult;
use base::utils::rt::GlobalRuntime;

use crate::lease::LeaseService;
use crate::store::InMemoryGuardStore;

pub fn spawn(
    runtime: &GlobalRuntime,
    store: InMemoryGuardStore,
) -> GlobalResult<base::tokio::task::JoinHandle<()>> {
    let cancel = runtime.cancel.clone();
    runtime.spawn("guard-lease-expirer", async move {
        let mut interval = base::tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(base::tokio::time::MissedTickBehavior::Delay);
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {}
            }
            let expired = LeaseService::new(store.clone()).expire_due(now_ms());
            if !expired.is_empty() {
                base::log::debug!("guard allocation leases expired: {}", expired.join(","));
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
