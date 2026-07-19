use std::collections::HashSet;
use std::time::Duration;

use crate::api::v2::control::BusinessControl;
use crate::store::InMemoryGuardStore;
use crate::store::model::PlaybackPresenceRecord;

const SCAN_INTERVAL: Duration = Duration::from_secs(1);
const CLEANUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const TERMINAL_RETENTION_MS: i64 = 3_600_000;

pub fn spawn(store: InMemoryGuardStore) -> base::tokio::task::JoinHandle<()> {
    base::tokio::spawn(async move {
        let mut interval = base::tokio::time::interval(SCAN_INTERVAL);
        loop {
            interval.tick().await;
            let now_ms = now_ms();
            store.prune_terminal_playback_presences(now_ms);
            for presence in store.claim_expired_playback_presences(now_ms) {
                let cleanup_store = store.clone();
                base::tokio::spawn(async move {
                    cleanup_expired_presence(cleanup_store, presence).await;
                });
            }
        }
    })
}

async fn cleanup_expired_presence(store: InMemoryGuardStore, presence: PlaybackPresenceRecord) {
    let cleanup_id = format!(
        "playback-presence-expire-{}-{}",
        presence.playback_id, presence.generation
    );
    let output_ids = store
        .playback_tickets_for_subscription(&presence.stream_id, &presence.subscription_id)
        .into_iter()
        .map(|ticket| ticket.output_id)
        .filter(|output_id| !output_id.is_empty())
        .collect::<HashSet<_>>();
    let control = BusinessControl::new(store.clone());

    loop {
        let mut outputs_closed = true;
        for output_id in &output_ids {
            let operation_id = format!("{cleanup_id}-output-{output_id}");
            if control
                .close_stream_output(&operation_id, &presence.stream_id, output_id)
                .await
                .is_err()
            {
                outputs_closed = false;
            }
        }

        let released = control
            .release_stream(
                &format!("{cleanup_id}-release"),
                &presence.stream_id,
                &presence.subscription_id,
            )
            .await
            .is_ok();
        if outputs_closed && released {
            store.revoke_playback_tickets_for_subscription(
                &presence.stream_id,
                &presence.subscription_id,
            );
            store.finish_playback_presence_cleanup(
                &presence.playback_id,
                &presence.stream_id,
                presence.generation,
                now_ms().saturating_add(TERMINAL_RETENTION_MS),
            );
            base::log::debug!(
                "playback presence expired: action=playback_presence_cleanup, outcome=closed, reason=heartbeat_timeout, stream_id={}, playback_id={}, generation={}",
                presence.stream_id,
                presence.playback_id,
                presence.generation
            );
            return;
        }
        base::tokio::time::sleep(CLEANUP_RETRY_INTERVAL).await;
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
