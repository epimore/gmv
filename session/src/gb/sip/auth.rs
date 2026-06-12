use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base::bytes::Bytes;
use base::dashmap::DashMap;
use base::exception::GlobalResult;
use base::tokio::sync::Mutex as AsyncMutex;
use base::tokio::time::{self, Instant};
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::message::{HeaderMapExt, extract_user_from_uri_like};
use gmv_pjsip::parser::parse_sip_message;
use gmv_pjsip::{AuthRequirement, PasswordProvider, SipMethod};

use crate::storage::entity::GmvOauth;

static DEVICE_AUTH_CACHE: OnceLock<Arc<DeviceAuthCache>> = OnceLock::new();

const POSITIVE_CACHE_TTL: Duration = Duration::from_secs(300);
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
const CACHE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct AuthCacheEntry {
    oauth: Option<GmvOauth>,
    expires_at: Instant,
}

#[derive(Default)]
pub struct DeviceAuthCache {
    entries: DashMap<String, AuthCacheEntry>,
    load_locks: DashMap<String, Arc<AsyncMutex<()>>>,
}

impl DeviceAuthCache {
    fn cached(&self, device_id: &str) -> Option<Option<GmvOauth>> {
        let entry = self.entries.get(device_id)?;
        if entry.expires_at > Instant::now() {
            return Some(entry.oauth.clone());
        }
        drop(entry);
        self.entries
            .remove_if(device_id, |_, entry| entry.expires_at <= Instant::now());
        None
    }

    pub fn get(&self, device_id: &str) -> Option<GmvOauth> {
        self.cached(device_id).flatten()
    }

    pub async fn get_or_load(&self, device_id: &str) -> GlobalResult<Option<GmvOauth>> {
        if let Some(cached) = self.cached(device_id) {
            return Ok(cached);
        }

        let load_lock = self
            .load_locks
            .entry(device_id.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let guard = load_lock.lock().await;

        let oauth = if let Some(cached) = self.cached(device_id) {
            cached
        } else {
            let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id).await?;
            let ttl = if oauth.is_some() {
                POSITIVE_CACHE_TTL
            } else {
                NEGATIVE_CACHE_TTL
            };
            self.entries.insert(
                device_id.to_owned(),
                AuthCacheEntry {
                    oauth: oauth.clone(),
                    expires_at: Instant::now() + ttl,
                },
            );
            oauth
        };

        drop(guard);
        self.load_locks.remove_if(device_id, |_, current| {
            Arc::ptr_eq(current, &load_lock) && Arc::strong_count(current) == 2
        });
        Ok(oauth)
    }

    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.expires_at > now);
        self.load_locks
            .retain(|_, lock| Arc::strong_count(lock) > 1);
    }
}

impl PasswordProvider for DeviceAuthCache {
    fn requirement_for(&self, username: &str, _realm: &str) -> AuthRequirement {
        let Some(entry) = self.get(username) else {
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

    fn password_for(&self, username: &str, _realm: &str) -> Option<String> {
        self.get(username)
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
    if let Some(cache) = global() {
        cache.get_or_load(&device_id).await?;
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
    use super::{AuthCacheEntry, DeviceAuthCache, POSITIVE_CACHE_TTL};
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
            "34020000001110000009".into(),
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
            "34020000001110000009".into(),
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
}
