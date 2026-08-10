use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::integration::model::{
    Integration, IntegrationHttpConfig, IntegrationMapping, IntegrationTransport,
};
use gmv_guard_server::mqttc::{CommandIdRepository, MqttCommandPolicy};
use gmv_guard_server::outbox::OutboxRepository;
use gmv_guard_server::runtime::event_forwarder::EventForwarder;
use gmv_guard_server::store::command::{HttpCommandClaim, http_command_id};
use gmv_guard_server::store::persistent::{CommandRepository, IntegrationRepository};
use gmv_guard_server::store::sqlite::SqliteStore;

#[test]
fn sqlite_http_idempotency_survives_reopen_and_rejects_request_drift() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-http-idempotency-{}.db",
                uuid::Uuid::new_v4()
            ));
            let pool_config = DatabasePoolConfig {
                max_size: 1,
                min_idle: Some(0),
                ..DatabasePoolConfig::default()
            };
            let store = SqliteStore::new(
                build_sqlite_pool(SqliteConnectionConfig::new(&path), pool_config.clone()).unwrap(),
            );
            store.migrate().await.unwrap();
            let repository = CommandRepository::Sqlite(store.clone());
            let command_id = http_command_id("app-1", "request-1");
            assert!(matches!(
                repository
                    .claim_http(
                        &command_id,
                        "app-1",
                        "request-1",
                        "POST /openapi/v1/streams/stream-1/stop",
                        "hash-1",
                        86_400_100,
                        100,
                    )
                    .await
                    .unwrap(),
                HttpCommandClaim::Claimed { .. }
            ));
            repository
                .complete_http(&command_id, 200, br#"{"accepted":true}"#, 101)
                .await
                .unwrap();
            drop(repository);
            drop(store);

            let reopened = SqliteStore::new(
                build_sqlite_pool(SqliteConnectionConfig::new(&path), pool_config).unwrap(),
            );
            reopened.migrate().await.unwrap();
            let repository = CommandRepository::Sqlite(reopened.clone());
            match repository
                .claim_http(
                    &command_id,
                    "app-1",
                    "request-1",
                    "POST /openapi/v1/streams/stream-1/stop",
                    "hash-1",
                    86_400_100,
                    102,
                )
                .await
                .unwrap()
            {
                HttpCommandClaim::Completed {
                    status,
                    response_body,
                    ..
                } => {
                    assert_eq!(status, 200);
                    assert_eq!(response_body, br#"{"accepted":true}"#);
                }
                other => panic!("expected completed command, got {other:?}"),
            }
            assert!(
                repository
                    .claim_http(
                        &command_id,
                        "app-1",
                        "request-1",
                        "POST /openapi/v1/streams/stream-1/stop",
                        "different-hash",
                        86_400_100,
                        103,
                    )
                    .await
                    .is_err()
            );
            drop(repository);
            drop(reopened);
            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn mqtt_authorization_uses_current_integration_state_and_exact_topic() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-mqtt-authorization-{}.db",
                uuid::Uuid::new_v4()
            ));
            let store = SqliteStore::new(
                build_sqlite_pool(
                    SqliteConnectionConfig::new(&path),
                    DatabasePoolConfig {
                        max_size: 1,
                        min_idle: Some(0),
                        ..DatabasePoolConfig::default()
                    },
                )
                .unwrap(),
            );
            store.migrate().await.unwrap();
            let integrations = IntegrationRepository::Sqlite(store.clone());
            let mut integration = Integration {
                integration_id: "app-1".to_string(),
                name: "MQTT app".to_string(),
                transport: IntegrationTransport::Mqtt,
                inbound_enabled: true,
                outbound_enabled: false,
                enabled: true,
                scopes: vec!["streams:write".to_string()],
                expires_at_ms: None,
                config_version: 1,
                created_by: "test".to_string(),
                created_at_ms: 100,
                updated_at_ms: 100,
            };
            integrations.upsert(&integration).await.unwrap();
            let policy = MqttCommandPolicy::new(["stream.stop".to_string()], 60_000).unwrap();
            let commands = CommandIdRepository::from(store.clone());
            let payload = br#"{
              "integration_id":"app-1",
              "command_id":"cmd-1",
              "issued_at_ms":1000,
              "expires_at_ms":2000,
              "action":"stream.stop",
              "target":"stream-1"
            }"#;
            assert!(
                policy
                    .decode_authorized_topic_with_repository(
                        "gmv/commands/app-1",
                        payload,
                        1500,
                        &commands,
                        &integrations,
                    )
                    .await
                    .unwrap()
                    .is_some()
            );
            let unknown_topic = payload.replace_ascii(b"cmd-1", b"cmd-2");
            assert!(
                policy
                    .decode_authorized_topic_with_repository(
                        "gmv/commands/unknown",
                        &unknown_topic,
                        1500,
                        &commands,
                        &integrations,
                    )
                    .await
                    .is_err()
            );
            integration.scopes.clear();
            integration.updated_at_ms = 150;
            integrations.upsert(&integration).await.unwrap();
            let scope_revoked = payload.replace_ascii(b"cmd-1", b"cmd-4");
            assert!(
                policy
                    .decode_authorized_topic_with_repository(
                        "gmv/commands/app-1",
                        &scope_revoked,
                        1500,
                        &commands,
                        &integrations,
                    )
                    .await
                    .is_err()
            );
            integration.scopes.push("streams:write".to_string());
            integration.enabled = false;
            integration.updated_at_ms = 200;
            integrations.upsert(&integration).await.unwrap();
            let disabled = payload.replace_ascii(b"cmd-1", b"cmd-3");
            assert!(
                policy
                    .decode_authorized_topic_with_repository(
                        "gmv/commands/app-1",
                        &disabled,
                        1500,
                        &commands,
                        &integrations,
                    )
                    .await
                    .is_err()
            );
            drop(commands);
            drop(integrations);
            drop(store);
            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn http_callback_only_enqueues_documented_business_events() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-http-callback-contract-{}.db",
                uuid::Uuid::new_v4()
            ));
            let store = SqliteStore::new(
                build_sqlite_pool(
                    SqliteConnectionConfig::new(&path),
                    DatabasePoolConfig {
                        max_size: 1,
                        min_idle: Some(0),
                        ..DatabasePoolConfig::default()
                    },
                )
                .unwrap(),
            );
            store.migrate().await.unwrap();
            let integrations = IntegrationRepository::Sqlite(store.clone());
            integrations
                .upsert(&Integration {
                    integration_id: "app-http".to_string(),
                    name: "HTTP app".to_string(),
                    transport: IntegrationTransport::Http,
                    inbound_enabled: false,
                    outbound_enabled: true,
                    enabled: true,
                    scopes: Vec::new(),
                    expires_at_ms: None,
                    config_version: 1,
                    created_by: "test".to_string(),
                    created_at_ms: 100,
                    updated_at_ms: 100,
                })
                .await
                .unwrap();
            integrations
                .upsert_http_config(&IntegrationHttpConfig {
                    integration_id: "app-http".to_string(),
                    callback_url: Some("https://partner.example.test/gmv/events".to_string()),
                    callback_timeout_ms: 5_000,
                    private_network_policy: "deny".to_string(),
                    private_network_allowlist: Vec::new(),
                    max_attempts: 5,
                    event_ttl_ms: 86_400_000,
                    max_response_bytes: 65_536,
                    updated_at_ms: 100,
                })
                .await
                .unwrap();
            integrations
                .upsert_mapping(&IntegrationMapping {
                    mapping_id: "mapping-all".to_string(),
                    integration_id: "app-http".to_string(),
                    direction: "OUTBOUND".to_string(),
                    source_type: "**".to_string(),
                    schema_version: "v1".to_string(),
                    destination_kind: "HTTP".to_string(),
                    destination: "https://stale.example.test/events".to_string(),
                    payload_profile: "event-envelope-v1".to_string(),
                    enabled: true,
                    created_at_ms: 100,
                    updated_at_ms: 100,
                })
                .await
                .unwrap();

            let outbox = OutboxRepository::from(store.clone());
            let forwarder = EventForwarder::new(outbox.clone(), Vec::new())
                .with_integrations(integrations.clone());
            forwarder
                .forward(
                    "event-public".to_string(),
                    "session.alarm".to_string(),
                    br#"{"deviceId":"device-1"}"#.to_vec(),
                )
                .await
                .unwrap();
            forwarder
                .forward(
                    "event-internal".to_string(),
                    "stream.registered.fallback".to_string(),
                    b"diagnostic".to_vec(),
                )
                .await
                .unwrap();

            let records = outbox.list(10).await.unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].event_id, "event-public");
            assert_eq!(
                records[0].destination,
                "https://partner.example.test/gmv/events/session/alarm"
            );
            let envelope: base::serde_json::Value =
                base::serde_json::from_slice(&records[0].payload).unwrap();
            assert_eq!(envelope["event_type"], "session.alarm");
            assert_eq!(envelope["schema_version"], "v1");

            drop(forwarder);
            drop(outbox);
            drop(integrations);
            drop(store);
            let _ = std::fs::remove_file(path);
        });
}

trait ReplaceAscii {
    fn replace_ascii(&self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl ReplaceAscii for [u8] {
    fn replace_ascii(&self, from: &[u8], to: &[u8]) -> Vec<u8> {
        assert_eq!(from.len(), to.len());
        let mut output = self.to_vec();
        let index = output
            .windows(from.len())
            .position(|window| window == from)
            .expect("test payload must contain source text");
        output[index..index + from.len()].copy_from_slice(to);
        output
    }
}
