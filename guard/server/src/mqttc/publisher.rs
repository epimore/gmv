use std::future::Future;
use std::pin::Pin;

use base_rpc::RetryPolicy;
use rumqttc::{AsyncClient, QoS};

use crate::core::{GuardError, GuardResult};
use crate::outbox::OutboxDelivery;
use crate::store::model::{OutboxDestinationKind, OutboxRecord};

#[derive(Clone)]
pub struct MqttPublisher {
    client: AsyncClient,
    retry: RetryPolicy,
}

impl MqttPublisher {
    pub fn new(client: AsyncClient, retry: RetryPolicy) -> Self {
        Self { client, retry }
    }

    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry
    }

    pub async fn publish(&self, topic: &str, payload: &[u8]) -> GuardResult<()> {
        if topic.is_empty() || topic.contains(['#', '+']) {
            return Err(GuardError::InvalidConfig(
                "MQTT publish topic must be concrete".to_string(),
            ));
        }
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await
            .map_err(|error| GuardError::Conflict(format!("MQTT publish failed: {error}")))
    }
}

impl OutboxDelivery for MqttPublisher {
    fn deliver<'a>(
        &'a self,
        record: &'a OutboxRecord,
    ) -> Pin<Box<dyn Future<Output = GuardResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if record.destination_kind != OutboxDestinationKind::Mqtt {
                return Err(GuardError::InvalidConfig(
                    "MQTT publisher received non-MQTT outbox record".to_string(),
                ));
            }
            self.publish(&record.destination, &record.payload).await
        })
    }
}
