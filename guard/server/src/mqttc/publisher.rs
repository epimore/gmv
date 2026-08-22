use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base_rpc::RetryPolicy;
use parking_lot::RwLock;
use rumqttc::v5::AsyncClient as AsyncClientV5;
use rumqttc::v5::mqttbytes::QoS as QoSV5;
use rumqttc::{AsyncClient, QoS};

use crate::core::{GuardError, GuardResult};
use crate::outbox::OutboxDelivery;
use crate::store::model::{OutboxDestinationKind, OutboxRecord};

#[derive(Clone)]
pub struct MqttPublisher {
    client: Arc<RwLock<Option<MqttPublishClient>>>,
    retry: RetryPolicy,
}

#[derive(Clone)]
pub enum MqttPublishClient {
    V3(AsyncClient),
    V5(AsyncClientV5),
}

impl MqttPublisher {
    pub fn new(client: MqttPublishClient, retry: RetryPolicy) -> Self {
        Self {
            client: Arc::new(RwLock::new(Some(client))),
            retry,
        }
    }

    pub fn disconnected(retry: RetryPolicy) -> Self {
        Self {
            client: Arc::new(RwLock::new(None)),
            retry,
        }
    }

    pub fn replace_from(&self, publisher: &Self) {
        *self.client.write() = publisher.client.read().clone();
    }

    pub fn disconnect(&self) {
        *self.client.write() = None;
    }

    pub fn is_available(&self) -> bool {
        self.client.read().is_some()
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
        let client = self
            .client
            .read()
            .clone()
            .ok_or_else(|| GuardError::Conflict("MQTT runtime is not connected".to_string()))?;
        match client {
            MqttPublishClient::V3(client) => client
                .publish(topic, QoS::AtLeastOnce, false, payload)
                .await
                .map_err(|error| GuardError::Conflict(format!("MQTT v3 publish failed: {error}"))),
            MqttPublishClient::V5(client) => client
                .publish(topic, QoSV5::AtLeastOnce, false, payload.to_vec())
                .await
                .map_err(|error| GuardError::Conflict(format!("MQTT v5 publish failed: {error}"))),
        }
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
