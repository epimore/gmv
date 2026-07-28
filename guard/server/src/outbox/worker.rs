use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base_rpc::RetryPolicy;
use parking_lot::Mutex;

use crate::core::GuardResult;
use crate::outbox::state::{mark_dead, mark_delivered, mark_retry, mark_sending};
use crate::store::InMemoryGuardStore;
use crate::store::model::{OutboxDestinationKind, OutboxRecord};
#[cfg(feature = "db-mysql")]
use crate::store::mysql::MysqlStore;
#[cfg(feature = "db-sqlite")]
use crate::store::sqlite::SqliteStore;

const DEAD_RETENTION_MS: i64 = 72 * 60 * 60 * 1_000;
const MAX_DEAD_PER_INTEGRATION: usize = 1_000;

pub trait OutboxDelivery: Send + Sync {
    fn deliver<'a>(
        &'a self,
        record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>>;
}

#[derive(Clone, Default)]
pub struct DeliveryRouter {
    deliveries: HashMap<OutboxDestinationKind, Arc<dyn OutboxDelivery>>,
}

impl DeliveryRouter {
    pub fn with(mut self, kind: OutboxDestinationKind, delivery: Arc<dyn OutboxDelivery>) -> Self {
        self.deliveries.insert(kind, delivery);
        self
    }
}

impl OutboxDelivery for DeliveryRouter {
    fn deliver<'a>(
        &'a self,
        record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let delivery = self
                .deliveries
                .get(&record.destination_kind)
                .ok_or_else(|| {
                    crate::core::GuardError::InvalidConfig(format!(
                        "no delivery registered for {:?}",
                        record.destination_kind
                    ))
                })?;
            delivery.deliver(record).await
        })
    }
}

#[derive(Debug, Clone)]
pub enum OutboxRepository {
    Memory(InMemoryGuardStore),
    #[cfg(feature = "db-mysql")]
    Mysql(MysqlStore),
    #[cfg(feature = "db-sqlite")]
    Sqlite(SqliteStore),
}

impl From<InMemoryGuardStore> for OutboxRepository {
    fn from(store: InMemoryGuardStore) -> Self {
        Self::Memory(store)
    }
}
#[cfg(feature = "db-mysql")]
impl From<MysqlStore> for OutboxRepository {
    fn from(store: MysqlStore) -> Self {
        Self::Mysql(store)
    }
}
#[cfg(feature = "db-sqlite")]
impl From<SqliteStore> for OutboxRepository {
    fn from(store: SqliteStore) -> Self {
        Self::Sqlite(store)
    }
}

impl OutboxRepository {
    pub async fn insert_outbox_records(&self, records: Vec<OutboxRecord>) -> GuardResult<()> {
        match self {
            Self::Memory(store) => store.insert_outbox_records(records),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.insert_outbox_records(&records).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.insert_outbox_records(&records).await,
        }
    }

    pub async fn insert_mapped_outbox_records(
        &self,
        records: Vec<OutboxRecord>,
    ) -> GuardResult<()> {
        match self {
            Self::Memory(store) => store.insert_mapped_outbox_records(records),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.insert_mapped_outbox_records(&records).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.insert_mapped_outbox_records(&records).await,
        }
    }

    pub async fn list(&self, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        match self {
            Self::Memory(store) => Ok(store.outbox_records(limit)),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.outbox_records(limit).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.outbox_records(limit).await,
        }
    }

    pub async fn retry_dead(&self, outbox_id: &str, now_ms: i64) -> GuardResult<OutboxRecord> {
        match self {
            Self::Memory(store) => store.retry_dead_outbox(outbox_id, now_ms),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.retry_dead_outbox(outbox_id, now_ms).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.retry_dead_outbox(outbox_id, now_ms).await,
        }
    }

    async fn due(&self, now_ms: i64, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        match self {
            Self::Memory(store) => Ok(store.due_outbox(now_ms, limit)),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.due_outbox(now_ms, limit).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.due_outbox(now_ms, limit).await,
        }
    }

    async fn recover_stale_sending(&self, stale_before_ms: i64, now_ms: i64) -> GuardResult<()> {
        match self {
            Self::Memory(store) => {
                store.recover_stale_sending(stale_before_ms, now_ms);
                Ok(())
            }
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => {
                store.recover_stale_sending(stale_before_ms, now_ms).await?;
                Ok(())
            }
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => {
                store.recover_stale_sending(stale_before_ms, now_ms).await?;
                Ok(())
            }
        }
    }

    async fn update(&self, record: OutboxRecord) -> GuardResult<()> {
        match self {
            Self::Memory(store) => store.update_outbox(record),
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.update_outbox(&record).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.update_outbox(&record).await,
        }
    }

    async fn delete(&self, outbox_id: &str) -> GuardResult<()> {
        match self {
            Self::Memory(store) => {
                store.remove_outbox(outbox_id);
                Ok(())
            }
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => store.delete_outbox(outbox_id).await,
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => store.delete_outbox(outbox_id).await,
        }
    }

    async fn cleanup_dead(&self, now_ms: i64) -> GuardResult<()> {
        let older_than_ms = now_ms.saturating_sub(DEAD_RETENTION_MS);
        match self {
            Self::Memory(store) => {
                store.cleanup_dead_outbox(older_than_ms, MAX_DEAD_PER_INTEGRATION);
                Ok(())
            }
            #[cfg(feature = "db-mysql")]
            Self::Mysql(store) => {
                store
                    .cleanup_dead_outbox(older_than_ms, MAX_DEAD_PER_INTEGRATION)
                    .await?;
                Ok(())
            }
            #[cfg(feature = "db-sqlite")]
            Self::Sqlite(store) => {
                store
                    .cleanup_dead_outbox(older_than_ms, MAX_DEAD_PER_INTEGRATION)
                    .await?;
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
pub struct OutboxWorker {
    store: OutboxRepository,
    delivery: Arc<dyn OutboxDelivery>,
    retry: RetryPolicy,
    batch_size: usize,
    sending_timeout: Duration,
    max_record_age: Option<Duration>,
    delete_delivered: bool,
    delivery_failure_episode: Arc<Mutex<FailureEpisode>>,
}

impl OutboxWorker {
    pub fn new(
        store: impl Into<OutboxRepository>,
        delivery: Arc<dyn OutboxDelivery>,
        retry: RetryPolicy,
        batch_size: usize,
    ) -> Self {
        Self {
            store: store.into(),
            delivery,
            retry,
            batch_size: batch_size.max(1),
            sending_timeout: Duration::from_secs(30),
            max_record_age: None,
            delete_delivered: false,
            delivery_failure_episode: Arc::new(Mutex::new(FailureEpisode::default())),
        }
    }

    pub fn with_sending_timeout(mut self, timeout: Duration) -> Self {
        if !timeout.is_zero() {
            self.sending_timeout = timeout;
        }
        self
    }

    pub fn with_max_record_age(mut self, age: Duration) -> Self {
        if !age.is_zero() {
            self.max_record_age = Some(age);
        }
        self
    }

    pub fn with_delete_delivered(mut self, enabled: bool) -> Self {
        self.delete_delivered = enabled;
        self
    }

    pub async fn run_once(&self, now_ms: i64) -> GuardResult<usize> {
        let timeout_ms = self.sending_timeout.as_millis().min(i64::MAX as u128) as i64;
        self.store
            .recover_stale_sending(now_ms.saturating_sub(timeout_ms), now_ms)
            .await?;
        let records = self.store.due(now_ms, self.batch_size).await?;
        let mut failed_deliveries = 0usize;
        let mut delivered = 0usize;
        for mut record in records.iter().cloned() {
            mark_sending(&mut record, now_ms)?;
            self.store.update(record.clone()).await?;
            if self.record_expired(&record, now_ms) {
                mark_dead(&mut record, now_ms, "outbox record expired before delivery")?;
                base::log::warn!(
                    "guard outbox transition: outbox_id={}, event_id={}, outcome=dead, reason=expired, attempts={}",
                    record.outbox_id,
                    record.event_id,
                    record.attempts
                );
                self.store.update(record).await?;
                continue;
            }
            match self.delivery.deliver(&record).await {
                Ok(()) => {
                    mark_delivered(&mut record, now_ms)?;
                    delivered = delivered.saturating_add(1);
                    base::log::debug!(
                        "guard outbox transition: outbox_id={}, event_id={}, outcome=delivered, attempts={}",
                        record.outbox_id,
                        record.event_id,
                        record.attempts
                    );
                    if self.delete_delivered {
                        self.store.delete(&record.outbox_id).await?;
                        continue;
                    }
                }
                Err(error)
                    if delivery_error_is_retryable(&error)
                        && self.retry.permits(record.attempts.saturating_add(1)) =>
                {
                    failed_deliveries = failed_deliveries.saturating_add(1);
                    let reason = error.to_string();
                    let delay = self.retry.delay(record.attempts);
                    let next =
                        now_ms.saturating_add(delay.as_millis().min(i64::MAX as u128) as i64);
                    mark_retry(&mut record, now_ms, next, reason)?;
                    base::log::trace!(
                        "guard outbox transition: outbox_id={}, event_id={}, outcome=retry_wait, attempts={}, next_attempt_at_ms={}, reason=delivery_failed",
                        record.outbox_id,
                        record.event_id,
                        record.attempts,
                        record.next_attempt_at_ms
                    );
                }
                Err(error) => {
                    failed_deliveries = failed_deliveries.saturating_add(1);
                    let reason = error.to_string();
                    mark_dead(&mut record, now_ms, reason)?;
                    base::log::warn!(
                        "guard outbox transition: outbox_id={}, event_id={}, outcome=dead, attempts={}, reason=delivery_failed",
                        record.outbox_id,
                        record.event_id,
                        record.attempts
                    );
                }
            }
            self.store.update(record).await?;
        }
        self.store.cleanup_dead(now_ms).await?;
        self.record_delivery_result(failed_deliveries, delivered);
        Ok(records.len())
    }

    fn record_delivery_result(&self, failed_deliveries: usize, delivered: usize) {
        let mut episode = self.delivery_failure_episode.lock();
        if failed_deliveries > 0 {
            match episode.record_failure(Instant::now()) {
                EpisodeDecision::Started => base::log::warn!(
                    "guard outbox delivery state changed: state=failed, previous_state=ready, outcome=retrying, failed_records={failed_deliveries}"
                ),
                EpisodeDecision::Summary {
                    total,
                    since_last_summary,
                    suppressed,
                    duration,
                } => base::log::warn!(
                    "guard outbox delivery remains failed: state=failed, outcome=ongoing, failed_records={failed_deliveries}, failure_batches={total}, batches_since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
                    duration.as_millis()
                ),
                EpisodeDecision::Suppressed => {}
                EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
            }
        } else if delivered > 0
            && let EpisodeDecision::Recovered {
                total,
                suppressed,
                duration,
            } = episode.record_success(Instant::now())
        {
            base::log::info!(
                "guard outbox delivery state changed: state=ready, previous_state=failed, outcome=recovered, delivered_records={delivered}, failure_batches={total}, suppressed={suppressed}, duration_ms={}",
                duration.as_millis()
            );
        }
    }

    fn record_expired(&self, record: &OutboxRecord, now_ms: i64) -> bool {
        if record
            .expires_at_ms
            .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
        {
            return true;
        }
        let Some(max_age) = self.max_record_age else {
            return false;
        };
        let max_age_ms = max_age.as_millis().min(i64::MAX as u128) as i64;
        now_ms.saturating_sub(record.created_at_ms) > max_age_ms
    }
}

fn delivery_error_is_retryable(error: &crate::core::GuardError) -> bool {
    matches!(
        error,
        crate::core::GuardError::Conflict(_)
            | crate::core::GuardError::Capacity(_)
            | crate::core::GuardError::TimeUnsynced(_)
            | crate::core::GuardError::UserVisible {
                retryable: true,
                ..
            }
    )
}
