use std::sync::Arc;
use std::time::Duration;

use base::cache::c100k::{Cache, CacheEvent};
use base::exception::GlobalResult;
use base::once_cell::sync::OnceCell;
use base::tokio;
use base::tokio::runtime::Handle;
use base::tokio_util::sync::CancellationToken;

use crate::register::core::TimeScheduleKey;

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum ScheduleKey {
    Register(TimeScheduleKey),
    Transaction(String),
    GeneralCache(String),
}

#[derive(Clone)]
pub struct ScheduleEvent<K> {
    pub key: K,
    pub hash: u64,
    pub version: u32,
}

#[derive(Clone)]
pub struct TimeScheduler {
    inner: Arc<Cache<ScheduleKey>>,
}

static TIME_SCHEDULER: OnceCell<TimeScheduler> = OnceCell::new();

impl TimeScheduler {
    pub fn init() -> &'static Self {
        TIME_SCHEDULER.get_or_init(|| {
            let handle = Handle::current();
            let _enter = handle.enter();
            Self {
                inner: Arc::new(Cache::default()),
            }
        })
    }

    pub fn global() -> &'static Self {
        TIME_SCHEDULER.get().expect("TimeScheduler not initialized")
    }

    pub fn try_global() -> Option<&'static Self> {
        TIME_SCHEDULER.get()
    }

    pub fn insert_register(&self, key: TimeScheduleKey, ttl: Duration) -> GlobalResult<()> {
        self.inner.insert(ScheduleKey::Register(key), ttl)
    }

    pub fn refresh_register(&self, key: &TimeScheduleKey) -> GlobalResult<()> {
        self.inner.refresh(ScheduleKey::Register(key.clone()))
    }

    pub fn remove_register(&self, key: &TimeScheduleKey) -> GlobalResult<()> {
        self.inner.delete(ScheduleKey::Register(key.clone()))
    }

    pub fn insert_transaction(&self, key: String, ttl: Duration) -> GlobalResult<()> {
        self.inner.insert(ScheduleKey::Transaction(key), ttl)
    }

    pub fn refresh_transaction(&self, key: &str) -> GlobalResult<()> {
        self.inner
            .refresh(ScheduleKey::Transaction(key.to_string()))
    }

    pub fn remove_transaction(&self, key: &str) -> GlobalResult<()> {
        self.inner.delete(ScheduleKey::Transaction(key.to_string()))
    }

    pub fn insert_general_cache(&self, key: String, ttl: Duration) -> GlobalResult<()> {
        self.inner.insert(ScheduleKey::GeneralCache(key), ttl)
    }

    pub fn remove_general_cache(&self, key: &str) -> GlobalResult<()> {
        self.inner
            .delete(ScheduleKey::GeneralCache(key.to_string()))
    }

    pub async fn next_batch(
        &self,
        cancel_token: &CancellationToken,
    ) -> Option<Vec<ScheduleEvent<ScheduleKey>>> {
        tokio::select! {
            batch = self.inner.next_batch() => {
                batch.map(|items| {
                    items.into_iter()
                        .map(|CacheEvent { key, hash, version }| ScheduleEvent { key, hash, version })
                        .collect()
                })
            }
            _ = cancel_token.cancelled() => None,
        }
    }
}
