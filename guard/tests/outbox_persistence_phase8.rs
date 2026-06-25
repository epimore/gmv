use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use base_rpc::RetryPolicy;
use guard::core::GuardResult;
use guard::mqttc::{CommandIdRepository, MqttCommandPolicy};
use guard::outbox::{OutboxDelivery, OutboxWorker};
use guard::store::model::{EventRecord, OutboxDestinationKind, OutboxRecord, OutboxState};
use guard::store::sqlite::SqliteStore;

struct Success;
impl OutboxDelivery for Success {
    fn deliver<'a>(
        &'a self,
        _record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn sqlite_outbox_survives_pool_reopen_and_resumes_delivery() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path =
                std::env::temp_dir().join(format!("guard-outbox-{}.db", uuid::Uuid::new_v4()));
            let pool_config = DatabasePoolConfig {
                max_size: 1,
                min_idle: Some(0),
                ..DatabasePoolConfig::default()
            };
            let pool =
                build_sqlite_pool(SqliteConnectionConfig::new(&path), pool_config.clone()).unwrap();
            let store = SqliteStore::new(pool);
            store.migrate().await.unwrap();
            let event = EventRecord {
                event_id: "e1".to_string(),
                topic: "node.health".to_string(),
                priority: 1,
                payload: b"{}".to_vec(),
            };
            let record = OutboxRecord {
                outbox_id: "o1".to_string(),
                event_id: "e1".to_string(),
                destination_kind: OutboxDestinationKind::Mqtt,
                destination: "gmv/events".to_string(),
                payload: b"{}".to_vec(),
                state: OutboxState::Pending,
                attempts: 0,
                next_attempt_at_ms: 100,
                last_error: None,
                created_at_ms: 100,
                updated_at_ms: 100,
            };
            assert!(
                store
                    .insert_event_with_outbox(&event, &[record])
                    .await
                    .unwrap()
            );
            drop(store);

            let pool = build_sqlite_pool(SqliteConnectionConfig::new(&path), pool_config).unwrap();
            let reopened = SqliteStore::new(pool);
            reopened.migrate().await.unwrap();
            let policy = MqttCommandPolicy::new(["stream.stop".to_string()], 60_000).unwrap();
            let command = br#"{"command_id":"cmd-1","issued_at_ms":100,"expires_at_ms":1000,"action":"stream.stop","target":"stream-1"}"#;
            let commands = CommandIdRepository::from(reopened.clone());
            assert!(
                policy
                    .decode_with_repository(command, 100, &commands)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert!(
                policy
                    .decode_with_repository(command, 100, &commands)
                    .await
                    .unwrap()
                    .is_none()
            );
            drop(commands);
            drop(reopened);

            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            let reopened = SqliteStore::new(pool);
            reopened.migrate().await.unwrap();
            let commands = CommandIdRepository::from(reopened.clone());
            assert!(
                policy
                    .decode_with_repository(command, 100, &commands)
                    .await
                    .unwrap()
                    .is_none()
            );
            let worker = OutboxWorker::new(
                reopened.clone(),
                Arc::new(Success),
                RetryPolicy {
                    initial_delay: Duration::from_secs(1),
                    max_delay: Duration::from_secs(4),
                    multiplier: 2.0,
                    jitter_ratio: 0.0,
                    max_attempts: Some(3),
                },
                10,
            );
            assert_eq!(worker.run_once(100).await.unwrap(), 1);
            assert_eq!(
                reopened.get_outbox("o1").await.unwrap().state,
                OutboxState::Delivered
            );
            reopened
                .retry_dead_outbox("missing", 200)
                .await
                .unwrap_err();
            drop(reopened);
            let _ = std::fs::remove_file(path);
        });
}
