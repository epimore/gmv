use crate::bus::router::topic_matches;
use crate::core::GuardResult;
use crate::outbox::OutboxRepository;
use crate::store::model::{OutboxDestinationKind, OutboxRecord, OutboxState};

#[derive(Debug, Clone)]
pub struct EventForwardRule {
    pub pattern: String,
    pub topic_prefix: String,
}

#[derive(Debug, Clone)]
pub struct EventForwarder {
    repository: OutboxRepository,
    rules: Vec<EventForwardRule>,
}

impl EventForwarder {
    pub fn new(repository: OutboxRepository, rules: Vec<EventForwardRule>) -> Self {
        Self { repository, rules }
    }

    pub async fn forward(
        &self,
        event_id: String,
        topic: String,
        payload: Vec<u8>,
    ) -> GuardResult<()> {
        let mut records = Vec::new();
        let now = now_ms();
        for rule in &self.rules {
            if !topic_matches(&rule.pattern, &topic) {
                continue;
            }
            let mqtt_topic = mqtt_topic(&rule.topic_prefix, &topic);
            records.push(OutboxRecord {
                outbox_id: format!("mqtt-{event_id}-{}", records.len() + 1),
                event_id: event_id.clone(),
                destination_kind: OutboxDestinationKind::Mqtt,
                destination: mqtt_topic,
                payload: payload.clone(),
                state: OutboxState::Pending,
                attempts: 0,
                next_attempt_at_ms: now,
                last_error: None,
                created_at_ms: now,
                updated_at_ms: now,
            });
        }
        if records.is_empty() {
            return Ok(());
        }
        self.repository.insert_outbox_records(records).await
    }
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
