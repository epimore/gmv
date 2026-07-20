use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base::log::{debug, error};
use base::once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::register::core::{Register, TimeScheduleKey};
use crate::service::{stream_close, stream_rpc};
use crate::state::StreamNodeRegistry;
use crate::state::session::Cache;

pub const PLAYBACK_PRESENCE_TTL_MS: i64 = 181_000;
const CLEANUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const TERMINAL_RETENTION_MS: i64 = 3_600_000;

static PRESENCES: Lazy<Mutex<PresenceStore>> = Lazy::new(|| Mutex::new(PresenceStore::default()));

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackPresence {
    playback_id: String,
    stream_id: String,
    subscription_id: String,
    generation: u64,
    expires_at_ms: i64,
    control_in_flight: bool,
    closing: bool,
    terminal: bool,
}

#[derive(Debug, Default)]
struct PresenceStore {
    records: HashMap<String, PlaybackPresence>,
}

impl PresenceStore {
    fn upsert(
        &mut self,
        playback_id: &str,
        stream_id: &str,
        subscription_id: &str,
        generation: u64,
        expires_at_ms: i64,
    ) {
        self.prune(expires_at_ms.saturating_sub(PLAYBACK_PRESENCE_TTL_MS));
        self.records.insert(
            playback_id.to_string(),
            PlaybackPresence {
                playback_id: playback_id.to_string(),
                stream_id: stream_id.to_string(),
                subscription_id: subscription_id.to_string(),
                generation,
                expires_at_ms,
                control_in_flight: false,
                closing: false,
                terminal: false,
            },
        );
    }

    fn refresh(
        &mut self,
        playback_id: &str,
        stream_id: &str,
        subscription_id: &str,
        generation: u64,
        now_ms: i64,
    ) -> Option<i64> {
        self.prune(now_ms);
        let presence = self.records.get_mut(playback_id)?;
        if presence.closing
            || presence.control_in_flight
            || presence.expires_at_ms <= now_ms
            || presence.stream_id != stream_id
            || presence.generation != generation
            || (!presence.subscription_id.is_empty() && presence.subscription_id != subscription_id)
        {
            return None;
        }
        if presence.subscription_id.is_empty() {
            presence.subscription_id = subscription_id.to_string();
        }
        presence.expires_at_ms = now_ms.saturating_add(PLAYBACK_PRESENCE_TTL_MS);
        Some(presence.expires_at_ms)
    }

    fn begin_control(
        &mut self,
        playback_id: &str,
        stream_id: &str,
        generation: u64,
        now_ms: i64,
    ) -> bool {
        self.prune(now_ms);
        let Some(presence) = self.records.get_mut(playback_id) else {
            return true;
        };
        if presence.stream_id != stream_id
            || presence.generation != generation
            || presence.expires_at_ms <= now_ms
            || presence.closing
            || presence.control_in_flight
        {
            return false;
        }
        presence.control_in_flight = true;
        true
    }

    fn finish_control(&mut self, playback_id: &str, stream_id: &str, generation: u64) {
        if let Some(presence) = self.records.get_mut(playback_id)
            && presence.stream_id == stream_id
            && presence.generation == generation
            && !presence.closing
        {
            presence.control_in_flight = false;
        }
    }

    fn remove(&mut self, playback_id: &str, stream_id: &str) -> Option<PlaybackPresence> {
        if self
            .records
            .get(playback_id)
            .is_some_and(|presence| presence.stream_id == stream_id)
        {
            self.records.remove(playback_id)
        } else {
            None
        }
    }

    fn remove_for_subscription(
        &mut self,
        stream_id: &str,
        subscription_id: &str,
    ) -> Vec<PlaybackPresence> {
        let playback_ids = self
            .records
            .values()
            .filter(|presence| {
                presence.stream_id == stream_id && presence.subscription_id == subscription_id
            })
            .map(|presence| presence.playback_id.clone())
            .collect::<Vec<_>>();
        playback_ids
            .into_iter()
            .filter_map(|playback_id| self.records.remove(&playback_id))
            .collect()
    }

    fn remove_for_stream(&mut self, stream_id: &str) -> Vec<PlaybackPresence> {
        let playback_ids = self
            .records
            .values()
            .filter(|presence| presence.stream_id == stream_id)
            .map(|presence| presence.playback_id.clone())
            .collect::<Vec<_>>();
        playback_ids
            .into_iter()
            .filter_map(|playback_id| self.records.remove(&playback_id))
            .collect()
    }

    fn claim_expired(
        &mut self,
        playback_id: &str,
        generation: u64,
        now_ms: i64,
    ) -> Option<PlaybackPresence> {
        self.prune(now_ms);
        let presence = self.records.get_mut(playback_id)?;
        if presence.generation != generation
            || presence.expires_at_ms > now_ms
            || presence.control_in_flight
            || presence.closing
        {
            return None;
        }
        presence.closing = true;
        Some(presence.clone())
    }

    fn finish_terminal(&mut self, presence: &PlaybackPresence, terminal_until_ms: i64) {
        if let Some(current) = self.records.get_mut(&presence.playback_id)
            && current.stream_id == presence.stream_id
            && current.generation == presence.generation
            && current.closing
        {
            current.expires_at_ms = terminal_until_ms;
            current.terminal = true;
        } else if !self.records.contains_key(&presence.playback_id) {
            let mut terminal = presence.clone();
            terminal.expires_at_ms = terminal_until_ms;
            terminal.control_in_flight = false;
            terminal.closing = true;
            terminal.terminal = true;
            self.records.insert(terminal.playback_id.clone(), terminal);
        }
    }

    fn expiry_retry_delay_ms(
        &self,
        playback_id: &str,
        generation: u64,
        now_ms: i64,
    ) -> Option<i64> {
        let presence = self.records.get(playback_id)?;
        if presence.generation != generation || presence.closing || presence.terminal {
            return None;
        }
        if presence.control_in_flight {
            return Some(1_000);
        }
        (presence.expires_at_ms > now_ms).then(|| presence.expires_at_ms - now_ms)
    }

    fn prune(&mut self, now_ms: i64) {
        self.records
            .retain(|_, presence| !presence.terminal || presence.expires_at_ms > now_ms);
    }
}

pub fn initialize(
    playback_id: &str,
    stream_id: &str,
    subscription_id: &str,
    generation: u64,
) -> Option<i64> {
    if playback_id.is_empty() || stream_id.is_empty() {
        return None;
    }
    let expires_at_ms = now_ms().saturating_add(PLAYBACK_PRESENCE_TTL_MS);
    PRESENCES.lock().upsert(
        playback_id,
        stream_id,
        subscription_id,
        generation,
        expires_at_ms,
    );
    schedule(playback_id, generation, PLAYBACK_PRESENCE_TTL_MS);
    Some(expires_at_ms)
}

pub fn restore(playback_id: &str, stream_id: &str, generation: u64) -> Option<i64> {
    initialize(playback_id, stream_id, "", generation)
}

pub fn refresh(
    playback_id: &str,
    stream_id: &str,
    subscription_id: &str,
    generation: u64,
    now_ms: i64,
) -> Option<i64> {
    let deadline =
        PRESENCES
            .lock()
            .refresh(playback_id, stream_id, subscription_id, generation, now_ms)?;
    if !Cache::stream_map_contains_token(&stream_id.to_string(), &subscription_id.to_string())
        && !Cache::stream_map_insert_token(stream_id.to_string(), subscription_id.to_string())
    {
        clear(playback_id, stream_id);
        return None;
    }
    schedule(playback_id, generation, deadline.saturating_sub(now_ms));
    Some(deadline)
}

pub fn begin_control(playback_id: &str, stream_id: &str, generation: u64, now_ms: i64) -> bool {
    PRESENCES
        .lock()
        .begin_control(playback_id, stream_id, generation, now_ms)
}

pub struct ControlGuard {
    playback_id: String,
    stream_id: String,
    generation: u64,
}

impl Drop for ControlGuard {
    fn drop(&mut self) {
        finish_control(&self.playback_id, &self.stream_id, self.generation);
    }
}

pub fn acquire_control(
    playback_id: &str,
    stream_id: &str,
    generation: u64,
) -> Option<ControlGuard> {
    begin_control(playback_id, stream_id, generation, now_ms()).then(|| ControlGuard {
        playback_id: playback_id.to_string(),
        stream_id: stream_id.to_string(),
        generation,
    })
}

pub fn finish_control(playback_id: &str, stream_id: &str, generation: u64) {
    PRESENCES
        .lock()
        .finish_control(playback_id, stream_id, generation);
}

pub fn clear(playback_id: &str, stream_id: &str) {
    if let Some(presence) = PRESENCES.lock().remove(playback_id, stream_id) {
        cancel(&presence);
    }
}

pub fn clear_for_subscription(stream_id: &str, subscription_id: &str) {
    for presence in PRESENCES
        .lock()
        .remove_for_subscription(stream_id, subscription_id)
    {
        cancel(&presence);
    }
}

pub fn clear_for_stream(stream_id: &str) {
    for presence in PRESENCES.lock().remove_for_stream(stream_id) {
        cancel(&presence);
    }
}

pub async fn expire(playback_id: Arc<str>, generation: u64) {
    let current_time_ms = now_ms();
    let (presence, retry_delay_ms) = {
        let mut presences = PRESENCES.lock();
        let presence = presences.claim_expired(&playback_id, generation, current_time_ms);
        let retry_delay_ms = presence
            .is_none()
            .then(|| presences.expiry_retry_delay_ms(&playback_id, generation, current_time_ms));
        (presence, retry_delay_ms.flatten())
    };
    let Some(presence) = presence else {
        if let Some(retry_delay_ms) = retry_delay_ms {
            schedule(&playback_id, generation, retry_delay_ms);
        }
        return;
    };
    loop {
        match cleanup_subscription(&presence).await {
            Ok(stream_stopped) => {
                PRESENCES
                    .lock()
                    .finish_terminal(&presence, now_ms().saturating_add(TERMINAL_RETENTION_MS));
                publish_terminal(&presence, stream_stopped);
                debug!(
                    "playback presence expired: action=playback_presence_cleanup, outcome=closed, reason=heartbeat_timeout, stream_id={}, playback_id={}, generation={}",
                    presence.stream_id, presence.playback_id, presence.generation
                );
                return;
            }
            Err(err) => {
                debug!(
                    "playback presence cleanup retry: stream_id={}, playback_id={}, generation={}, err={err}",
                    presence.stream_id, presence.playback_id, presence.generation
                );
                base::tokio::time::sleep(CLEANUP_RETRY_INTERVAL).await;
            }
        }
    }
}

async fn cleanup_subscription(presence: &PlaybackPresence) -> Result<bool, String> {
    if presence.subscription_id.is_empty() {
        stream_close::begin(presence.stream_id.clone());
        return Ok(true);
    }
    if let Some((node_id, _)) = Cache::stream_map_query_node(&presence.stream_id) {
        let node = match StreamNodeRegistry::get(&node_id) {
            Some(node) => node,
            None => crate::guard_integration::ensure_stream_node(&node_id)
                .await
                .map_err(|err| err.to_string())?,
        };
        stream_rpc::release_subscription_outputs(
            &node,
            &format!(
                "playback-presence-expire-{}-{}",
                presence.playback_id, presence.generation
            ),
            &presence.stream_id,
            &presence.subscription_id,
        )
        .await
        .map_err(|err| err.to_string())?;
    }
    let remaining = Cache::stream_map_release_token(&presence.stream_id, &presence.subscription_id);
    let stopped = remaining.is_none_or(|remaining| remaining == 0);
    if stopped {
        stream_close::begin(presence.stream_id.clone());
    }
    Ok(stopped)
}

fn publish_terminal(presence: &PlaybackPresence, stream_stopped: bool) {
    let payload = base::serde_json::json!({
        "playback_id": presence.playback_id,
        "stream_id": presence.stream_id,
        "subscription_id": presence.subscription_id,
        "generation": presence.generation,
        "stream_stopped": stream_stopped,
        "reason": "heartbeat_timeout"
    });
    match base::serde_json::to_vec(&payload) {
        Ok(payload) => crate::guard_integration::publish_guard_event(
            "session.playback_presence_terminal",
            payload,
        ),
        Err(err) => error!(
            "serialize playback presence terminal event failed: stream_id={}, playback_id={}, err={err}",
            presence.stream_id, presence.playback_id
        ),
    }
}

fn schedule(playback_id: &str, generation: u64, delay_ms: i64) {
    let key = TimeScheduleKey::PlaybackPresenceExpiry(Arc::from(playback_id), generation);
    let _ = Register::scheduler().remove_register(&key);
    let delay = Duration::from_millis(u64::try_from(delay_ms.max(1)).unwrap_or(u64::MAX));
    if let Err(err) = Register::scheduler().insert_register(key, delay) {
        error!(
            "schedule playback presence deadline failed: outcome=close_subscription, playback_id={playback_id}, generation={generation}, err={err}"
        );
        let playback_id = Arc::<str>::from(playback_id);
        base::tokio::spawn(async move {
            base::tokio::time::sleep(delay).await;
            expire(playback_id, generation).await;
        });
    }
}

fn cancel(presence: &PlaybackPresence) {
    let _ = Register::scheduler().remove_register(&TimeScheduleKey::PlaybackPresenceExpiry(
        Arc::from(presence.playback_id.as_str()),
        presence.generation,
    ));
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::{PLAYBACK_PRESENCE_TTL_MS, PresenceStore};

    fn store() -> PresenceStore {
        let mut store = PresenceStore::default();
        store.upsert(
            "playback-a",
            "stream-a",
            "subscription-a",
            7,
            PLAYBACK_PRESENCE_TTL_MS,
        );
        store
    }

    #[test]
    fn refresh_uses_last_accepted_heartbeat_and_strict_boundary() {
        let mut store = store();
        assert_eq!(
            store.refresh("playback-a", "stream-a", "subscription-a", 7, 120_000,),
            Some(301_000)
        );
        assert!(store.claim_expired("playback-a", 7, 300_999).is_none());
        assert!(store.claim_expired("playback-a", 7, 301_000).is_some());
    }

    #[test]
    fn expiry_and_control_have_one_winner() {
        let mut store = store();
        assert!(store.begin_control("playback-a", "stream-a", 7, 180_999));
        assert!(store.claim_expired("playback-a", 7, 181_000).is_none());
        assert_eq!(
            store.expiry_retry_delay_ms("playback-a", 7, 181_000),
            Some(1_000)
        );
        store.finish_control("playback-a", "stream-a", 7);
        assert!(store.claim_expired("playback-a", 7, 181_000).is_some());
        assert!(!store.begin_control("playback-a", "stream-a", 7, 181_001));
    }

    #[test]
    fn terminal_tombstone_survives_stream_close_removal() {
        let mut store = store();
        let presence = store
            .claim_expired("playback-a", 7, PLAYBACK_PRESENCE_TTL_MS)
            .unwrap();
        assert!(store.remove_for_stream("stream-a").len() == 1);
        store.finish_terminal(&presence, 3_781_000);

        assert!(store.records["playback-a"].terminal);
        assert!(!store.begin_control("playback-a", "stream-a", 7, 181_001));
        assert!(
            store
                .refresh("playback-a", "stream-a", "subscription-a", 7, 181_001,)
                .is_none()
        );
    }

    #[test]
    fn restored_presence_binds_first_authenticated_subscription() {
        let mut store = PresenceStore::default();
        store.upsert("playback-a", "stream-a", "", 7, 181_000);
        assert_eq!(
            store.refresh("playback-a", "stream-a", "subscription-a", 7, 60_000,),
            Some(241_000)
        );
        assert_eq!(
            store.records["playback-a"].subscription_id,
            "subscription-a"
        );
        assert!(
            store
                .refresh("playback-a", "stream-a", "subscription-b", 7, 120_000,)
                .is_none()
        );
    }
}
