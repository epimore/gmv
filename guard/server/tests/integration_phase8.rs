use std::time::Duration;

use guard::mqttc::{CommandAction, MqttClientConfig, MqttCommandPolicy};
use guard::webhook::signing;
use guard::webhook::{WebhookClient, WebhookUrlPolicy};

#[test]
fn mqtt_config_requires_complete_credentials_and_tls_is_explicit() {
    let config = MqttClientConfig {
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
            "device.talk".to_string(),
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
        ("device.talk", CommandAction::StreamTalk),
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
