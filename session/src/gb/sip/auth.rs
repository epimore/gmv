use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::exception::GlobalResult;
use base::tokio::sync::Mutex as AsyncMutex;
use base::tokio::time::{self, Instant};
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::auth::parse_digest_authorization;
use gmv_pjsip::message::{HeaderMapExt, extract_uri, extract_user_from_uri_like};
use gmv_pjsip::parser::parse_sip_message;
use gmv_pjsip::{AuthRequirement, PasswordProvider, SipMethod};

use crate::storage::entity::GmvOauth;

static DEVICE_AUTH_CACHE: OnceLock<Arc<DeviceAuthCache>> = OnceLock::new();

const POSITIVE_CACHE_TTL: Duration = Duration::from_secs(300);
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
const CACHE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_CACHE_CAPACITY: usize = 40_000;
pub const AUTH_DB_BATCH_LIMIT: usize = 2_000;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct AuthCacheKey {
    device_id: String,
    realm: String,
}

impl AuthCacheKey {
    fn new(device_id: &str, realm: &str) -> Self {
        Self {
            device_id: device_id.to_owned(),
            realm: realm.to_owned(),
        }
    }
}

#[derive(Clone)]
struct AuthCacheEntry {
    oauth: Option<GmvOauth>,
    expires_at: Instant,
}

pub struct DeviceAuthCache {
    entries: DashMap<AuthCacheKey, AuthCacheEntry>,
    load_locks: DashMap<AuthCacheKey, Arc<AsyncMutex<()>>>,
    max_entries: usize,
}

impl Default for DeviceAuthCache {
    fn default() -> Self {
        Self {
            entries: DashMap::new(),
            load_locks: DashMap::new(),
            max_entries: DEFAULT_CACHE_CAPACITY,
        }
    }
}

impl DeviceAuthCache {
    fn cached(&self, key: &AuthCacheKey) -> Option<Option<GmvOauth>> {
        let entry = self.entries.get(key)?;
        if entry.expires_at > Instant::now() {
            return Some(entry.oauth.clone());
        }
        drop(entry);
        self.entries
            .remove_if(key, |_, entry| entry.expires_at <= Instant::now());
        None
    }

    pub fn get(&self, device_id: &str, realm: &str) -> Option<GmvOauth> {
        self.cached(&AuthCacheKey::new(device_id, realm)).flatten()
    }

    pub fn get_by_device(&self, device_id: &str) -> Option<GmvOauth> {
        let now = Instant::now();
        self.entries.iter().find_map(|entry| {
            (entry.key().device_id == device_id && entry.expires_at > now)
                .then(|| entry.oauth.clone())
                .flatten()
        })
    }

    pub async fn get_or_load(
        &self,
        device_id: &str,
        realm: &str,
    ) -> GlobalResult<Option<GmvOauth>> {
        let mut loaded = self
            .get_or_load_many(&[(device_id.to_owned(), realm.to_owned())])
            .await?;
        Ok(loaded.pop().flatten())
    }

    pub async fn get_or_load_many(
        &self,
        lookups: &[(String, String)],
    ) -> GlobalResult<Vec<Option<GmvOauth>>> {
        if lookups.is_empty() {
            return Ok(Vec::new());
        }

        let requested = lookups
            .iter()
            .map(|(device_id, realm)| AuthCacheKey::new(device_id, realm))
            .collect::<Vec<_>>();
        let mut missing = requested
            .iter()
            .filter(|key| self.cached(key).is_none())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        missing.sort();

        let mut held_locks = Vec::with_capacity(missing.len());
        for key in &missing {
            let load_lock = self
                .load_locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone();
            let guard = load_lock.clone().lock_owned().await;
            held_locks.push((key.clone(), load_lock, guard));
        }

        let missing = missing
            .into_iter()
            .filter(|key| self.cached(key).is_none())
            .collect::<Vec<_>>();
        for chunk in missing.chunks(AUTH_DB_BATCH_LIMIT) {
            let device_ids = chunk
                .iter()
                .map(|key| key.device_id.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let oauth_by_device = GmvOauth::read_gmv_oauth_by_device_ids(&device_ids)
                .await?
                .into_iter()
                .map(|oauth| (oauth.device_id.clone(), oauth))
                .collect::<HashMap<_, _>>();
            let now = Instant::now();
            for key in chunk {
                let oauth = oauth_by_device
                    .get(&key.device_id)
                    .filter(|oauth| oauth.domain == key.realm)
                    .cloned();
                let ttl = if oauth.is_some() {
                    POSITIVE_CACHE_TTL
                } else {
                    NEGATIVE_CACHE_TTL
                };
                self.entries.insert(
                    key.clone(),
                    AuthCacheEntry {
                        oauth,
                        expires_at: now + ttl,
                    },
                );
            }
        }
        self.enforce_capacity();

        drop(held_locks);
        for key in &missing {
            self.load_locks
                .remove_if(key, |_, current| Arc::strong_count(current) == 1);
        }

        Ok(requested
            .iter()
            .map(|key| self.cached(key).flatten())
            .collect())
    }

    pub fn invalidate_device(&self, device_id: &str) {
        self.entries.retain(|key, _| key.device_id != device_id);
        self.load_locks.retain(|key, _| key.device_id != device_id);
    }

    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.expires_at > now);
        self.load_locks
            .retain(|_, lock| Arc::strong_count(lock) > 1);
    }

    fn enforce_capacity(&self) {
        self.cleanup_expired();
        let excess = self.entries.len().saturating_sub(self.max_entries);
        if excess == 0 {
            return;
        }

        let mut oldest = self
            .entries
            .iter()
            .map(|entry| (entry.key().clone(), entry.expires_at))
            .collect::<Vec<_>>();
        oldest.sort_by_key(|(_, expires_at)| *expires_at);
        for (key, _) in oldest.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }
}

impl PasswordProvider for DeviceAuthCache {
    fn requirement_for(&self, username: &str, realm: &str) -> AuthRequirement {
        let Some(entry) = self.get(username, realm) else {
            return AuthRequirement::Forbidden;
        };
        if entry.status == 0 {
            return AuthRequirement::Forbidden;
        }
        if entry.pwd_check == 0 {
            return AuthRequirement::Disabled;
        }
        if entry
            .pwd
            .as_deref()
            .is_some_and(|password| !password.is_empty())
        {
            AuthRequirement::Required
        } else {
            AuthRequirement::Forbidden
        }
    }

    fn password_for(&self, username: &str, realm: &str) -> Option<String> {
        self.get(username, realm)
            .filter(|entry| entry.status != 0 && entry.pwd_check != 0)
            .and_then(|entry| entry.pwd)
            .filter(|password| !password.is_empty())
    }
}

pub async fn init_global() -> GlobalResult<Arc<DeviceAuthCache>> {
    if let Some(cache) = DEVICE_AUTH_CACHE.get() {
        return Ok(cache.clone());
    }
    let cache = Arc::new(DeviceAuthCache::default());
    if DEVICE_AUTH_CACHE.set(cache.clone()).is_err() {
        return Ok(DEVICE_AUTH_CACHE
            .get()
            .expect("device auth cache initialized concurrently")
            .clone());
    }
    Ok(cache)
}

pub fn global() -> Option<&'static Arc<DeviceAuthCache>> {
    DEVICE_AUTH_CACHE.get()
}

pub fn invalidate_device(device_id: &str) {
    if let Some(cache) = global() {
        cache.invalidate_device(device_id);
    }
}

fn extract_realm_from_uri_like(value: &str) -> Option<String> {
    let uri = extract_uri(value)?;
    let value = uri.strip_prefix("sip:").unwrap_or(&uri);
    let (_, host) = value.split_once('@')?;
    let end = host.find([':', ';', '?']).unwrap_or(host.len());
    (end > 0).then(|| host[..end].to_owned())
}

pub async fn prepare_register(data: &Bytes) -> GlobalResult<()> {
    if data.len() < 9
        || !data[..8].eq_ignore_ascii_case(b"REGISTER")
        || !data[8].is_ascii_whitespace()
    {
        return Ok(());
    }
    let Ok(message) = parse_sip_message(data.clone()) else {
        return Ok(());
    };
    if !matches!(message.method(), Some(SipMethod::Register)) {
        return Ok(());
    }
    let Some(device_id) = message
        .header("From")
        .and_then(extract_user_from_uri_like)
        .or_else(|| {
            message
                .header("Contact")
                .and_then(extract_user_from_uri_like)
        })
    else {
        return Ok(());
    };
    let realm = message
        .header("Authorization")
        .and_then(|value| parse_digest_authorization(value).get("realm").cloned())
        .or_else(|| message.header("From").and_then(extract_realm_from_uri_like))
        .or_else(|| message.header("To").and_then(extract_realm_from_uri_like));
    let Some(realm) = realm else {
        return Ok(());
    };
    if let Some(cache) = global() {
        cache.get_or_load(&device_id, &realm).await?;
    }
    Ok(())
}

pub async fn run_cleanup_task(cancel_token: CancellationToken) {
    let mut ticker = time::interval(CACHE_CLEANUP_INTERVAL);
    ticker.tick().await;
    loop {
        base::tokio::select! {
            _ = ticker.tick() => {
                if let Some(cache) = global() {
                    cache.cleanup_expired();
                }
            }
            _ = cancel_token.cancelled() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthCacheEntry, AuthCacheKey, DeviceAuthCache, POSITIVE_CACHE_TTL};
    use crate::storage::entity::GmvOauth;
    use base::tokio::time::Instant;
    use gmv_pjsip::{AuthRequirement, PasswordProvider};

    fn oauth(pwd_check: u8, pwd: Option<&str>) -> GmvOauth {
        GmvOauth {
            device_id: "34020000001110000009".into(),
            domain_id: "34020000002000000001".into(),
            domain: "3402000000".into(),
            pwd: pwd.map(ToOwned::to_owned),
            pwd_check,
            alias: None,
            status: 1,
            heartbeat_sec: 60,
        }
    }

    #[test]
    fn password_policy_uses_cached_device_configuration() {
        let cache = DeviceAuthCache::default();
        cache.entries.insert(
            AuthCacheKey::new("34020000001110000009", "3402000000"),
            AuthCacheEntry {
                oauth: Some(oauth(1, Some("123456"))),
                expires_at: Instant::now() + POSITIVE_CACHE_TTL,
            },
        );

        assert_eq!(
            cache.requirement_for("34020000001110000009", "3402000000"),
            AuthRequirement::Required
        );
        assert_eq!(
            cache.password_for("34020000001110000009", "3402000000"),
            Some("123456".into())
        );
    }

    #[test]
    fn missing_or_password_disabled_device_does_not_require_digest() {
        let cache = DeviceAuthCache::default();
        cache.entries.insert(
            AuthCacheKey::new("34020000001110000009", "3402000000"),
            AuthCacheEntry {
                oauth: Some(oauth(0, None)),
                expires_at: Instant::now() + POSITIVE_CACHE_TTL,
            },
        );

        assert_eq!(
            cache.requirement_for("34020000001110000009", "3402000000"),
            AuthRequirement::Disabled
        );
        assert_eq!(
            cache.requirement_for("34020000001110000010", "3402000000"),
            AuthRequirement::Forbidden
        );
    }

    #[test]
    fn cache_keys_include_realm_and_support_device_invalidation() {
        let cache = DeviceAuthCache::default();
        for realm in ["3402000000", "4401000000"] {
            cache.entries.insert(
                AuthCacheKey::new("34020000001110000009", realm),
                AuthCacheEntry {
                    oauth: Some(oauth(1, Some("123456"))),
                    expires_at: Instant::now() + POSITIVE_CACHE_TTL,
                },
            );
        }

        assert_eq!(cache.entries.len(), 2);
        cache.invalidate_device("34020000001110000009");
        assert!(cache.entries.is_empty());
    }
}
