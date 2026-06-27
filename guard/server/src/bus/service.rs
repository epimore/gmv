use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::bus::queue::BoundedQueue;
use crate::bus::router::topic_matches;
use crate::core::{GuardError, GuardResult};
use crate::store::InMemoryGuardStore;
use crate::store::model::EventRecord;

pub type SubscriptionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusPriority {
    P3 = 0,
    P2 = 1,
    P1 = 2,
    P0 = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusEvent {
    pub event_id: String,
    pub topic: String,
    pub priority: BusPriority,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishOutcome {
    pub delivered: usize,
    pub duplicate: bool,
}

#[derive(Debug, Clone)]
pub struct BusService {
    inner: Arc<Mutex<BusInner>>,
    store: InMemoryGuardStore,
}

#[derive(Debug)]
struct BusInner {
    next_subscription_id: SubscriptionId,
    seen_events: HashSet<String>,
    subscriptions: HashMap<SubscriptionId, SubscriptionState>,
}

#[derive(Debug)]
struct SubscriptionState {
    pattern: String,
    queue: BoundedQueue<BusEvent>,
}

impl BusService {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BusInner {
                next_subscription_id: 1,
                seen_events: HashSet::new(),
                subscriptions: HashMap::new(),
            })),
            store,
        }
    }

    pub fn subscribe(
        &self,
        pattern: impl Into<String>,
        capacity: usize,
    ) -> GuardResult<SubscriptionId> {
        if capacity == 0 {
            return Err(GuardError::Capacity(
                "subscription capacity must be positive".to_string(),
            ));
        }
        let mut inner = self.inner.lock();
        let id = inner.next_subscription_id;
        inner.next_subscription_id += 1;
        inner.subscriptions.insert(
            id,
            SubscriptionState {
                pattern: pattern.into(),
                queue: BoundedQueue::new(capacity),
            },
        );
        Ok(id)
    }

    pub fn publish(&self, event: BusEvent) -> GuardResult<PublishOutcome> {
        let mut inner = self.inner.lock();
        if !inner.seen_events.insert(event.event_id.clone()) {
            return Ok(PublishOutcome {
                delivered: 0,
                duplicate: true,
            });
        }
        if matches!(event.priority, BusPriority::P0 | BusPriority::P1) {
            self.store.insert_event_once(EventRecord {
                event_id: event.event_id.clone(),
                topic: event.topic.clone(),
                priority: event.priority as u8,
                payload: event.payload.clone(),
            })?;
        }
        let mut delivered = 0;
        for subscription in inner.subscriptions.values_mut() {
            if topic_matches(&subscription.pattern, &event.topic) {
                match event.priority {
                    BusPriority::P0 | BusPriority::P1 => {
                        subscription.queue.try_push(event.clone()).map_err(|_| {
                            GuardError::Capacity(format!(
                                "subscription queue for pattern {} is full",
                                subscription.pattern
                            ))
                        })?
                    }
                    BusPriority::P2 | BusPriority::P3 => {
                        subscription.queue.push_drop_oldest(event.clone())
                    }
                }
                delivered += 1;
            }
        }
        Ok(PublishOutcome {
            delivered,
            duplicate: false,
        })
    }

    pub fn poll(&self, id: SubscriptionId) -> GuardResult<Option<BusEvent>> {
        let mut inner = self.inner.lock();
        let subscription = inner
            .subscriptions
            .get_mut(&id)
            .ok_or_else(|| GuardError::NotFound(format!("subscription {id}")))?;
        Ok(subscription.queue.pop())
    }

    pub fn queue_len(&self, id: SubscriptionId) -> GuardResult<usize> {
        let inner = self.inner.lock();
        let subscription = inner
            .subscriptions
            .get(&id)
            .ok_or_else(|| GuardError::NotFound(format!("subscription {id}")))?;
        Ok(subscription.queue.len())
    }
}
