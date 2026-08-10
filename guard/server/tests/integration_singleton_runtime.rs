#![cfg(feature = "db-sqlite")]

use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::integration::model::{
    Integration, IntegrationTransport, MqttRuntimeApplyState, MqttRuntimeRevision,
};
use gmv_guard_server::store::persistent::IntegrationRepository;
use gmv_guard_server::store::sqlite::SqliteStore;

fn integration(id: &str, created_at_ms: i64) -> Integration {
    Integration {
        integration_id: id.to_string(),
        name: id.to_string(),
        transport: IntegrationTransport::Mqtt,
        inbound_enabled: true,
        outbound_enabled: true,
        enabled: false,
        scopes: vec!["streams:read".to_string()],
        expires_at_ms: None,
        config_version: 1,
        created_by: "test".to_string(),
        created_at_ms,
        updated_at_ms: created_at_ms,
    }
}

fn revision() -> MqttRuntimeRevision {
    MqttRuntimeRevision {
        revision: 0,
        protocol_version: "v5".to_string(),
        broker: "broker.example.test".to_string(),
        port: 1883,
        client_id: "guard-test".to_string(),
        username: Some("guard".to_string()),
        password_ciphertext: Some("ciphertext-only".to_string()),
        tls: false,
        publish_event_ttl_sec: 86_400,
        created_by: "admin".to_string(),
        created_at_ms: 100,
    }
}

#[test]
fn mqtt_runtime_config_uses_cas_and_never_serializes_ciphertext() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-singleton-runtime-{}.db",
                uuid::Uuid::new_v4()
            ));
            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    connection_timeout: Duration::from_secs(2),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            let store = SqliteStore::new(pool.clone());
            store.migrate().await.unwrap();
            let repository = IntegrationRepository::Sqlite(store.clone());
            repository.upsert(&integration("app-1", 1)).await.unwrap();
            assert!(repository.upsert(&integration("app-2", 2)).await.is_err());
            let saved = repository
                .save_mqtt_runtime_config(&revision(), 0)
                .await
                .unwrap();
            assert_eq!(saved.desired_revision, 1);
            assert_eq!(saved.config_version, 1);
            assert_eq!(saved.apply_state, MqttRuntimeApplyState::Pending);
            assert!(saved.password_configured);
            let json = base::serde_json::to_string(&saved).unwrap();
            assert!(!json.contains("ciphertext-only"));
            assert!(
                repository
                    .save_mqtt_runtime_config(&revision(), 0)
                    .await
                    .is_err()
            );

            repository
                .update_mqtt_runtime_state(
                    1,
                    Some(1),
                    MqttRuntimeApplyState::Connected,
                    None,
                    None,
                    200,
                )
                .await
                .unwrap();
            let connected = repository.mqtt_runtime_config().await.unwrap().unwrap();
            assert_eq!(connected.active_revision, Some(1));
            assert_eq!(connected.apply_state, MqttRuntimeApplyState::Connected);

            let second = repository
                .save_mqtt_runtime_config(&revision(), 1)
                .await
                .unwrap();
            assert_eq!(second.desired_revision, 2);
            repository
                .update_mqtt_runtime_state(
                    2,
                    Some(1),
                    MqttRuntimeApplyState::Applying,
                    None,
                    None,
                    300,
                )
                .await
                .unwrap();
            let applying_revisions = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM guard_mqtt_runtime_revision WHERE slot='business'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(applying_revisions, 2);

            repository
                .update_mqtt_runtime_state(
                    2,
                    Some(2),
                    MqttRuntimeApplyState::Connected,
                    None,
                    None,
                    400,
                )
                .await
                .unwrap();
            let active_revisions = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT revision FROM guard_mqtt_runtime_revision WHERE slot='business' ORDER BY revision",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(active_revisions, [2]);

            let third = repository
                .save_mqtt_runtime_config(&revision(), 2)
                .await
                .unwrap();
            assert_eq!(third.desired_revision, 3);
            repository
                .update_mqtt_runtime_state(
                    3,
                    Some(2),
                    MqttRuntimeApplyState::Degraded,
                    Some("mqtt_connect_failed"),
                    Some("broker unavailable"),
                    500,
                )
                .await
                .unwrap();
            let rollback_revisions = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT revision FROM guard_mqtt_runtime_revision WHERE slot='business' ORDER BY revision",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(rollback_revisions, [2, 3]);

            for (index, state) in [
                "COMPLETED",
                "SUCCEEDED",
                "FAILED",
                "CANCELLED",
                "CLAIMED",
            ]
            .into_iter()
            .enumerate()
            {
                base_db::sqlx::query(
                    "INSERT INTO guard_command(command_id,expires_at_ms,created_at_ms,integration_id,state) VALUES (?,?,?,?,?)",
                )
                .bind(format!("command-{index}"))
                .bind(10_000_i64)
                .bind(index as i64)
                .bind("app-1")
                .bind(state)
                .execute(&pool)
                .await
                .unwrap();
            }
            assert_eq!(
                repository
                    .transport_switch_blockers("app-1")
                    .await
                    .unwrap(),
                (1, 0)
            );

            drop(repository);
            drop(store);
            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
}
