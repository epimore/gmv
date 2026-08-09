use std::collections::HashMap;
use std::time::Duration;

use gmv_guard_server::auth::{AuthState, Role, SessionPolicy};
use gmv_guard_server::mqttc::{
    CommandAction, MqttClientConfig, MqttCommandExecutor, MqttCommandPolicy, MqttProtocolVersion,
    RoutedCommand,
};
use gmv_guard_server::operation::OperationService;
use gmv_guard_server::outbox::OutboxRepository;
use gmv_guard_server::store::InMemoryGuardStore;
use gmv_guard_server::store::model::PlaybackTicketRecord;
use gmv_guard_server::webhook::signing;
use gmv_guard_server::webhook::{WebhookClient, WebhookUrlPolicy};

#[test]
fn mqtt_config_requires_complete_credentials_and_tls_is_explicit() {
    let config = MqttClientConfig {
        protocol_version: MqttProtocolVersion::V3,
        client_id: "guard-1".to_string(),
        host: "mqtt.example.com".to_string(),
        port: 8883,
        username: Some("guard".to_string()),
        password: None,
        keep_alive: Duration::from_secs(30),
        request_capacity: 100,
        tls: true,
        retry: base_rpc::RetryPolicy::default(),
    };
    assert!(config.validate().is_err());
}

#[test]
fn mqtt_commands_enforce_schema_ttl_permissions_and_idempotency() {
    let policy = MqttCommandPolicy::new(
        [
            "stream.stop".to_string(),
            "stream.playback".to_string(),
            "stream.download".to_string(),
            "device.broadcast".to_string(),
            "ai.start".to_string(),
            "ai.cancel".to_string(),
        ],
        60_000,
    )
    .unwrap();
    let payload = br#"{
      "command_id":"cmd-1",
      "issued_at_ms":1000,
      "expires_at_ms":2000,
      "action":"stream.stop",
      "target":"stream-1",
      "payload":{"reason":"manual"}
    }"#;
    let command = policy.decode(payload, 1500).unwrap().unwrap();
    assert_eq!(command.action, CommandAction::StreamStop);
    let operation = command.operation_request("mqtt");
    assert_eq!(operation.operation_id, "cmd-1");
    assert_eq!(operation.kind, "stream.stop");
    assert!(policy.decode(payload, 1500).unwrap().is_none());
    assert!(policy.decode(payload, 2001).is_err());

    for (action, expected) in [
        ("stream.playback", CommandAction::StreamPlayback),
        ("stream.download", CommandAction::StreamDownload),
        ("device.broadcast", CommandAction::DeviceBroadcast),
        ("ai.start", CommandAction::AiStart),
        ("ai.cancel", CommandAction::AiCancel),
    ] {
        let payload = format!(
            r#"{{
              "command_id":"cmd-{action}",
              "issued_at_ms":1000,
              "expires_at_ms":2000,
              "action":"{action}",
              "target":"target-1",
              "payload":{{"channel_id":"ch-1","model":"vehicle"}}
            }}"#
        );
        assert_eq!(
            policy
                .decode(payload.as_bytes(), 1500)
                .unwrap()
                .unwrap()
                .action,
            expected
        );
    }

    let forbidden = payload.replace_ascii(b"stream.stop", b"device.ptz ");
    assert!(policy.decode(&forbidden, 1500).is_err());
}

#[test]
fn mqtt_command_failure_queues_correlated_result() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            let executor = MqttCommandExecutor::new(OperationService::default(), store.clone())
                .with_result_outbox(
                    OutboxRepository::from(store.clone()),
                    HashMap::from([("app-1".to_string(), "gmv/command-results/app-1".to_string())]),
                );
            let error = executor
                .execute(RoutedCommand {
                    command_id: "cmd-result-1".to_string(),
                    integration_id: "app-1".to_string(),
                    expires_at_ms: i64::MAX,
                    action: CommandAction::StreamStart,
                    target: "device-1".to_string(),
                    payload: base::serde_json::json!({}),
                })
                .await
                .unwrap_err();
            assert!(error.to_string().contains("channel_id"));
            let records = store.outbox_records(10);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].destination, "gmv/command-results/app-1");
            let payload: base::serde_json::Value =
                base::serde_json::from_slice(&records[0].payload).unwrap();
            assert_eq!(payload["command_id"], "cmd-result-1");
            assert_eq!(payload["action"], "stream.start");
            assert_eq!(payload["state"], "failed");
            assert!(payload["result"].is_null());
        });
}

#[test]
fn mqtt_phase9_business_action_uses_shared_runtime_state_and_result_envelope() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            let executor = MqttCommandExecutor::new(OperationService::default(), store.clone())
                .with_result_outbox(
                    OutboxRepository::from(store.clone()),
                    HashMap::from([("app-1".to_string(), "gmv/command-results/app-1".to_string())]),
                );
            executor
                .execute(RoutedCommand {
                    command_id: "cmd-runtime-status-1".to_string(),
                    integration_id: "app-1".to_string(),
                    expires_at_ms: i64::MAX,
                    action: CommandAction::Business("runtime.status.get"),
                    target: "query".to_string(),
                    payload: base::serde_json::json!({}),
                })
                .await
                .unwrap();

            let records = store.outbox_records(10);
            assert_eq!(records.len(), 1);
            let payload: base::serde_json::Value =
                base::serde_json::from_slice(&records[0].payload).unwrap();
            assert_eq!(payload["action"], "runtime.status.get");
            assert_eq!(payload["state"], "succeeded");
            assert_eq!(payload["result"]["guard_available"], true);
            assert_eq!(payload["result"]["streams"], 0);
            assert_eq!(payload["result"]["ai_tasks"], 0);
        });
}

#[test]
fn every_phase9_mqtt_action_reaches_a_concrete_executor_branch() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let executor = MqttCommandExecutor::new(
                OperationService::default(),
                InMemoryGuardStore::default(),
            );
            for action in gmv_guard_server::integration::model::MQTT_COMMAND_ACTIONS
                .iter()
                .skip(9)
            {
                let result = executor
                    .execute(RoutedCommand {
                        command_id: format!("dispatch-{}", action.replace('.', "-")),
                        integration_id: "app-1".to_string(),
                        expires_at_ms: i64::MAX,
                        action: CommandAction::Business(action),
                        target: "target-1".to_string(),
                        payload: base::serde_json::json!({}),
                    })
                    .await;
                assert!(
                    result
                        .as_ref()
                        .err()
                        .is_none_or(|error| !error.to_string().contains("has no executor")),
                    "missing MQTT executor branch: {action}"
                );
            }
        });
}

#[test]
fn mqtt_playback_ticket_renewal_checks_owner_and_extends_lifecycle() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            let auth = AuthState::new(
                std::iter::empty::<gmv_guard_server::auth::UserAccount>(),
                SessionPolicy::default(),
            );
            let (service_token, _) = auth
                .issue_service_session("integration:app-1", Role::Admin, Duration::from_secs(300))
                .unwrap();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            store.upsert_playback_ticket(PlaybackTicketRecord {
                token: "ticket-1".to_string(),
                stream_id: "stream-1".to_string(),
                playback_id: "playback-1".to_string(),
                playback_start_time_sec: 0,
                playback_end_time_sec: 60,
                output_id: String::new(),
                subscription_id: "subscription-1".to_string(),
                lease_id: "lease-1".to_string(),
                route_id: "route-1".to_string(),
                username: "integration:app-1".to_string(),
                ui_session_token: service_token,
                required_role: Role::Viewer,
                issued_at_ms: now_ms,
                expires_at_ms: now_ms + 60_000,
                absolute_expires_at_ms: now_ms + 3_600_000,
                renewal_count: 0,
            });
            MqttCommandExecutor::new(OperationService::default(), store.clone())
                .with_auth(auth)
                .execute(RoutedCommand {
                    command_id: "renew-1".to_string(),
                    integration_id: "app-1".to_string(),
                    expires_at_ms: now_ms + 60_000,
                    action: CommandAction::PlaybackTicketRenew,
                    target: "ticket-1".to_string(),
                    payload: base::serde_json::json!({"renew": true}),
                })
                .await
                .unwrap();
            let ticket = store.get_playback_ticket("ticket-1").unwrap();
            assert!(ticket.expires_at_ms >= now_ms + 299_000);
            assert_eq!(ticket.renewal_count, 1);
        });
}

#[test]
fn webhook_hmac_is_stable_and_url_policy_rejects_ssrf_targets() {
    let signature = signing::sign(b"secret", 1234, br#"{"ok":true}"#).unwrap();
    assert_eq!(signature.len(), 64);
    assert_eq!(
        signature,
        signing::sign(b"secret", 1234, br#"{"ok":true}"#).unwrap()
    );

    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let client = WebhookClient::new(
                "secret",
                Duration::from_secs(2),
                1024,
                WebhookUrlPolicy::default(),
            )
            .unwrap();
            assert!(client.send("http://example.com/hook", b"{}").await.is_err());
            assert!(client.send("https://127.0.0.1/hook", b"{}").await.is_err());
            assert!(client.send("https://localhost/hook", b"{}").await.is_err());
        });
}

trait ReplaceAscii {
    fn replace_ascii(&self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl ReplaceAscii for [u8] {
    fn replace_ascii(&self, from: &[u8], to: &[u8]) -> Vec<u8> {
        assert_eq!(from.len(), to.len());
        let mut output = self.to_vec();
        if let Some(index) = output.windows(from.len()).position(|window| window == from) {
            output[index..index + from.len()].copy_from_slice(to);
        }
        output
    }
}
