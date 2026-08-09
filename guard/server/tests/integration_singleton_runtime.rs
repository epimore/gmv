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
            let store = SqliteStore::new(pool);
            store.migrate().await.unwrap();
            let repository = IntegrationRepository::Sqlite(store.clone());
            repository.upsert(&integration("app-1", 1)).await.unwrap();
            repository
                .bind_business_integration("app-1", "admin", 2)
                .await
                .unwrap();

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

            drop(repository);
            drop(store);
            let _ = std::fs::remove_file(path);
        });
}
