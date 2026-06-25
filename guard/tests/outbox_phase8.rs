use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base_rpc::RetryPolicy;
use guard::core::{GuardError, GuardResult};
use guard::outbox::{OutboxDelivery, OutboxWorker};
use guard::store::InMemoryGuardStore;
use guard::store::model::{EventRecord, OutboxDestinationKind, OutboxRecord, OutboxState};
use parking_lot::Mutex;

struct Delivery {
    results: Mutex<VecDeque<GuardResult<()>>>,
}

impl OutboxDelivery for Delivery {
    fn deliver<'a>(
        &'a self,
        _record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async move { self.results.lock().pop_front().unwrap_or(Ok(())) })
    }
}

fn outbox(id: &str, event_id: &str, now_ms: i64) -> OutboxRecord {
    OutboxRecord {
        outbox_id: id.to_string(),
        event_id: event_id.to_string(),
        destination_kind: OutboxDestinationKind::Mqtt,
        destination: "gmv/events".to_string(),
        payload: b"{}".to_vec(),
        state: OutboxState::Pending,
        attempts: 0,
        next_attempt_at_ms: now_ms,
        last_error: None,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    }
}

fn event(id: &str) -> EventRecord {
    EventRecord {
        event_id: id.to_string(),
        topic: "node.health".to_string(),
        priority: 1,
        payload: b"{}".to_vec(),
    }
}

fn worker(store: InMemoryGuardStore, results: Vec<GuardResult<()>>) -> OutboxWorker {
    OutboxWorker::new(
        store,
        Arc::new(Delivery {
            results: Mutex::new(results.into()),
        }),
        RetryPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(8),
            multiplier: 2.0,
            jitter_ratio: 0.0,
            max_attempts: Some(3),
        },
        100,
    )
}

#[test]
fn event_and_outbox_are_inserted_atomically_and_deduplicated() {
    let store = InMemoryGuardStore::default();
    assert!(
        store
            .insert_event_with_outbox(event("e1"), vec![outbox("o1", "e1", 0)])
            .unwrap()
    );
    assert!(
        !store
            .insert_event_with_outbox(event("e1"), vec![outbox("o2", "e1", 0)])
            .unwrap()
    );
    assert!(store.get_outbox("o1").is_some());
    assert!(store.get_outbox("o2").is_none());

    let error = store
        .insert_event_with_outbox(event("e2"), vec![outbox("o1", "e2", 0)])
        .unwrap_err();
    assert!(matches!(error, GuardError::Conflict(_)));
    assert!(store.events_after(Some("e1"), 10).is_empty());
}

#[test]
fn worker_retries_then_delivers_and_can_resume_from_store() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            store
                .insert_event_with_outbox(event("e1"), vec![outbox("o1", "e1", 100)])
                .unwrap();
            let first = worker(
                store.clone(),
                vec![Err(GuardError::Conflict("offline".to_string()))],
            );
            assert_eq!(first.run_once(100).await.unwrap(), 1);
            let record = store.get_outbox("o1").unwrap();
            assert_eq!(record.state, OutboxState::RetryWait);
            assert_eq!(record.attempts, 1);
            assert_eq!(record.next_attempt_at_ms, 1100);

            let resumed = worker(store.clone(), vec![Ok(())]);
            assert_eq!(resumed.run_once(1099).await.unwrap(), 0);
            assert_eq!(resumed.run_once(1100).await.unwrap(), 1);
            assert_eq!(
                store.get_outbox("o1").unwrap().state,
                OutboxState::Delivered
            );
        });
}

#[test]
fn worker_moves_to_dead_and_manual_retry_resets_record() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            store
                .insert_event_with_outbox(event("e1"), vec![outbox("o1", "e1", 0)])
                .unwrap();
            for now in [0, 1000, 3000] {
                worker(
                    store.clone(),
                    vec![Err(GuardError::Conflict("down".to_string()))],
                )
                .run_once(now)
                .await
                .unwrap();
            }
            assert_eq!(store.get_outbox("o1").unwrap().state, OutboxState::Dead);
            let retried = store.retry_dead_outbox("o1", 4000).unwrap();
            assert_eq!(retried.state, OutboxState::Pending);
            assert_eq!(retried.attempts, 0);
            worker(store.clone(), vec![Ok(())])
                .run_once(4000)
                .await
                .unwrap();
            assert_eq!(
                store.get_outbox("o1").unwrap().state,
                OutboxState::Delivered
            );
        });
}

#[test]
fn worker_recovers_stale_sending_after_crash() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            let mut record = outbox("o1", "e1", 0);
            record.state = OutboxState::Sending;
            record.attempts = 1;
            record.updated_at_ms = 0;
            store
                .insert_event_with_outbox(event("e1"), vec![record])
                .unwrap();
            let worker =
                worker(store.clone(), vec![Ok(())]).with_sending_timeout(Duration::from_secs(30));
            assert_eq!(worker.run_once(29_999).await.unwrap(), 0);
            assert_eq!(worker.run_once(30_000).await.unwrap(), 1);
            assert_eq!(
                store.get_outbox("o1").unwrap().state,
                OutboxState::Delivered
            );
            assert_eq!(store.get_outbox("o1").unwrap().attempts, 2);
        });
}
