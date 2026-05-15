use std::sync::Arc;
use std::time::Duration;

use base::cache::c100k::{Cache, CacheEvent, CacheKey};
use base::exception::GlobalResult;
use base::tokio;
use base::tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ScheduleEvent<K> {
    pub key: K,
    pub hash: u64,
    pub version: u32,
}

pub struct TimeScheduler<K: CacheKey> {
    inner: Arc<Cache<K>>,
}

impl<K: CacheKey> Clone for TimeScheduler<K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K: CacheKey> Default for TimeScheduler<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: CacheKey> TimeScheduler<K> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Cache::default()),
        }
    }

    pub fn insert(&self, key: K, ttl: Duration) -> GlobalResult<()> {
        self.inner.insert(key, ttl)
    }

    pub fn refresh(&self, key: &K) -> GlobalResult<()> {
        self.inner.refresh(key.clone())
    }

    pub fn remove(&self, key: &K) -> GlobalResult<()> {
        self.inner.delete(key.clone())
    }

    pub async fn next_batch(
        &self,
        cancel_token: &CancellationToken,
    ) -> Option<Vec<ScheduleEvent<K>>> {
        tokio::select! {
            batch = self.inner.next_batch() => {
                batch.map(|items| {
                    items.into_iter().map(|CacheEvent { key, hash, version }| {
                        ScheduleEvent { key, hash, version }
                    }).collect()
                })
            }
            _ = cancel_token.cancelled() => None,
        }
    }
}
