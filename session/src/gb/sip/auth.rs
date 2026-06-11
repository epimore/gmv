use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::tokio::time;
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::{AuthRequirement, PasswordProvider};
use parking_lot::RwLock;

use crate::storage::entity::GmvOauth;

static DEVICE_AUTH_CACHE: OnceLock<Arc<DeviceAuthCache>> = OnceLock::new();

#[derive(Default)]
pub struct DeviceAuthCache {
    entries: RwLock<HashMap<String, GmvOauth>>,
}

impl DeviceAuthCache {
    async fn reload(&self) -> GlobalResult<()> {
        let entries = GmvOauth::read_all_gmv_oauth().await?;
        *self.entries.write() = entries
            .into_iter()
            .map(|entry| (entry.device_id.clone(), entry))
            .collect();
        Ok(())
    }

    pub fn get(&self, device_id: &str) -> Option<GmvOauth> {
        self.entries.read().get(device_id).cloned()
    }
}

impl PasswordProvider for DeviceAuthCache {
    fn requirement_for(&self, username: &str, _realm: &str) -> AuthRequirement {
        let entries = self.entries.read();
        let Some(entry) = entries.get(username) else {
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
        self.entries
            .read()
            .get(username)
            .filter(|entry| entry.status != 0 && entry.pwd_check != 0)
            .and_then(|entry| entry.pwd.clone())
            .filter(|password| !password.is_empty())
    }
}

pub async fn init_global() -> GlobalResult<Arc<DeviceAuthCache>> {
    if let Some(cache) = DEVICE_AUTH_CACHE.get() {
        return Ok(cache.clone());
    }
    let cache = Arc::new(DeviceAuthCache::default());
    cache.reload().await?;
    if DEVICE_AUTH_CACHE.set(cache.clone()).is_err() {
        return DEVICE_AUTH_CACHE
            .get()
            .cloned()
            .ok_or_else(|| GlobalError::new_sys_error("device auth cache init failed", |_| {}));
    }
    Ok(cache)
}

pub fn global() -> Option<&'static Arc<DeviceAuthCache>> {
    DEVICE_AUTH_CACHE.get()
}

pub async fn run_refresh_task(cancel_token: CancellationToken) {
    let mut ticker = time::interval(Duration::from_secs(30));
    ticker.tick().await;
    loop {
        base::tokio::select! {
            _ = ticker.tick() => {
                if let Some(cache) = global() {
                    if let Err(err) = cache.reload().await {
                        warn!("refresh SIP device auth cache failed: {err}");
                    }
                } else {
                    error!("SIP device auth cache is not initialized");
                    break;
                }
            }
            _ = cancel_token.cancelled() => break,
        }
    }
}
