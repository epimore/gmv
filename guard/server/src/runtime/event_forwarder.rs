use crate::bus::router::topic_matches;
use crate::core::GuardResult;
use crate::outbox::OutboxRepository;
use crate::store::model::{OutboxDestinationKind, OutboxRecord, OutboxState};
use crate::store::persistent::IntegrationRepository;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct EventForwardRule {
    pub pattern: String,
    pub topic_prefix: String,
}

#[derive(Debug, Clone)]
pub struct EventForwarder {
    repository: OutboxRepository,
    rules: Vec<EventForwardRule>,
    integrations: Option<IntegrationRepository>,
}

impl EventForwarder {
    pub fn new(repository: OutboxRepository, rules: Vec<EventForwardRule>) -> Self {
        Self {
            repository,
            rules,
            integrations: None,
        }
    }

    pub fn with_integrations(mut self, integrations: IntegrationRepository) -> Self {
        self.integrations = Some(integrations);
        self
    }

    pub async fn forward(
        &self,
        event_id: String,
        topic: String,
        payload: Vec<u8>,
    ) -> GuardResult<()> {
        self.forward_inner(None, event_id, topic, payload).await
    }

    pub async fn forward_for_integration(
        &self,
        integration_id: &str,
        event_id: String,
        topic: String,
        payload: Vec<u8>,
    ) -> GuardResult<()> {
        self.forward_inner(Some(integration_id), event_id, topic, payload)
            .await
    }

    async fn forward_inner(
        &self,
        target_integration_id: Option<&str>,
        event_id: String,
        topic: String,
        payload: Vec<u8>,
    ) -> GuardResult<()> {
        let mut records = Vec::new();
        let now = now_ms();
        for (index, rule) in self
            .rules
            .iter()
            .enumerate()
            .filter(|_| target_integration_id.is_none())
        {
            if !topic_matches(&rule.pattern, &topic) {
                continue;
            }
            let mqtt_topic = mqtt_topic(&rule.topic_prefix, &topic);
            let mapping_id = format!("legacy-mqtt-{index}");
            records.push(OutboxRecord {
                outbox_id: delivery_id(&event_id, &mapping_id),
                event_id: event_id.clone(),
                integration_id: String::new(),
                mapping_id,
                destination_kind: OutboxDestinationKind::Mqtt,
                destination: mqtt_topic,
                payload: payload.clone(),
                state: OutboxState::Pending,
                attempts: 0,
                next_attempt_at_ms: now,
                last_error: None,
                created_at_ms: now,
                updated_at_ms: now,
                expires_at_ms: None,
            });
        }
        if let Some(integrations) = &self.integrations {
            let envelope = event_envelope(&event_id, &topic, &payload, now)?;
            for integration in integrations.list().await? {
                if target_integration_id.is_some_and(|target| target != integration.integration_id)
                {
                    continue;
                }
                if !integration.enabled
                    || !integration.outbound_enabled
                    || integration
                        .expires_at_ms
                        .is_some_and(|expires_at| expires_at <= now)
                {
                    continue;
                }
                for mapping in integrations
                    .list_mappings(&integration.integration_id)
                    .await?
                {
                    if !mapping.enabled
                        || mapping.direction != "OUTBOUND"
                        || !topic_matches(&mapping.source_type, &topic)
                    {
                        continue;
                    }
                    let (destination_kind, destination, expires_at_ms) =
                        match (integration.transport, mapping.destination_kind.as_str()) {
                            (crate::integration::model::IntegrationTransport::Http, "HTTP") => {
                                let Some(config) = integrations
                                    .http_config(&integration.integration_id)
                                    .await?
                                else {
                                    continue;
                                };
                                let Some(destination) = config.callback_url else {
                                    continue;
                                };
                                (
                                    OutboxDestinationKind::Webhook,
                                    destination,
                                    Some(now.saturating_add(config.event_ttl_ms)),
                                )
                            }
                            (crate::integration::model::IntegrationTransport::Mqtt, "MQTT") => {
                                (OutboxDestinationKind::Mqtt, mapping.destination, None)
                            }
                            _ => continue,
                        };
                    records.push(OutboxRecord {
                        outbox_id: delivery_id(&event_id, &mapping.mapping_id),
                        event_id: event_id.clone(),
                        integration_id: integration.integration_id.clone(),
                        mapping_id: mapping.mapping_id,
                        destination_kind,
                        destination,
                        payload: envelope.clone(),
                        state: OutboxState::Pending,
                        attempts: 0,
                        next_attempt_at_ms: now,
                        last_error: None,
                        created_at_ms: now,
                        updated_at_ms: now,
                        expires_at_ms,
                    });
                }
            }
        }
        if records.is_empty() {
            return Ok(());
        }
        self.repository.insert_mapped_outbox_records(records).await
    }
}

fn delivery_id(event_id: &str, mapping_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(event_id.as_bytes());
    digest.update([0]);
    digest.update(mapping_id.as_bytes());
    format!("delivery-{}", hex::encode(digest.finalize()))
}

fn event_envelope(
    event_id: &str,
    topic: &str,
    payload: &[u8],
    now_ms: i64,
) -> GuardResult<Vec<u8>> {
    if payload.len() > 1024 * 1024 {
        return Err(crate::core::GuardError::Capacity(
            "integration event payload exceeds 1 MiB".to_string(),
        ));
    }
    let payload =
        base::serde_json::from_slice::<base::serde_json::Value>(payload).unwrap_or_else(|_| {
            base::serde_json::Value::String(String::from_utf8_lossy(payload).into_owned())
        });
    base::serde_json::to_vec(&base::serde_json::json!({
        "event_id": event_id,
        "event_type": topic,
        "schema_version": "v1",
        "occurred_at_ms": now_ms,
        "payload": payload
    }))
    .map_err(|error| crate::core::GuardError::Conflict(format!("encode event envelope: {error}")))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn mqtt_topic(prefix: &str, topic: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let topic = topic.replace('.', "/");
    if prefix.is_empty() {
        topic
    } else {
        format!("{prefix}/{topic}")
    }
}

#[cfg(test)]
mod tests {
    use super::mqtt_topic;

    #[test]
    fn maps_dot_topic_to_mqtt_topic() {
        assert_eq!(
            mqtt_topic("gmv/events", "session.alarm"),
            "gmv/events/session/alarm"
        );
    }
}
