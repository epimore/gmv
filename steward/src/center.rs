use crate::{
    artifact::{ArtifactService, stable_error_code},
    config::CenterConfig,
    inventory::InventoryService,
    now_ms,
    state::StateStore,
    status::SharedStatus,
};
use base::tokio::sync::mpsc;
use base::tokio_util::sync::CancellationToken;
use base_mqtt::{
    MqttClientConfig, MqttEvent, MqttProtocolVersion, MqttPublisher, MqttQos, MqttReconnectPolicy,
    MqttRuntime, MqttSubscription, MqttTlsConfig, MqttWill,
};
use gmv_protocol::{
    common::v1::{NodeIdentity, NodeKind},
    guard::v1::CenterConnectionState,
    steward::v1::{
        CenterToStewardMessage, DeliveryReceipt, DeliveryState, StewardHeartbeat, StewardHello,
        StewardPresence, StewardToCenterMessage, center_to_steward_message,
        steward_to_center_message,
    },
    steward_mqtt::StewardTopics,
};
use prost::Message;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
pub struct CenterConnector {
    mqtt: MqttClientConfig,
    topics: StewardTopics,
    installation_id: String,
    node_id: String,
    instance_id: String,
    inventory: InventoryService,
    store: StateStore,
    status: SharedStatus,
    artifacts: ArtifactService,
    sequence: Arc<AtomicU64>,
}

pub struct CenterConnectorConfig {
    pub center: CenterConfig,
    pub installation_id: String,
    pub node_id: String,
    pub instance_id: String,
}

impl CenterConnector {
    pub fn new(
        config: CenterConnectorConfig,
        inventory: InventoryService,
        store: StateStore,
        status: SharedStatus,
        artifacts: ArtifactService,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let topics = StewardTopics::new(&config.center.topic_prefix, &config.installation_id)
            .map_err(io::Error::other)?;
        let sequence = Arc::new(AtomicU64::new(0));
        let offline = StewardToCenterMessage {
            message_id: uuid::Uuid::now_v7().to_string(),
            sequence: 1,
            sent_at_epoch_ms: now_ms(),
            installation_id: config.installation_id.clone(),
            steward_instance_id: config.instance_id.clone(),
            protocol_version: 1,
            payload: Some(steward_to_center_message::Payload::Presence(
                StewardPresence {
                    online: false,
                    observed_at_epoch_ms: now_ms(),
                },
            )),
        };
        let tls = config.center.tls.map(|tls| MqttTlsConfig {
            ca_certificate_path: Some(tls.ca_certificate_path),
            client_certificate_path: tls.client_certificate_path,
            client_private_key_path: tls.client_private_key_path,
        });
        let mqtt = MqttClientConfig {
            protocol_version: MqttProtocolVersion::V5,
            client_id: format!("steward-{}", config.installation_id),
            host: config.center.host,
            port: config.center.port,
            username: config.center.username,
            password: config.center.password,
            keep_alive: Duration::from_secs(config.center.keep_alive_secs),
            request_capacity: config.center.request_capacity,
            clean_start: false,
            session_expiry: Some(Duration::from_secs(config.center.session_expiry_secs)),
            tls,
            last_will: Some(MqttWill {
                topic: topics.presence(),
                payload: offline.encode_to_vec(),
                qos: MqttQos::AtLeastOnce,
                retain: true,
            }),
            reconnect: MqttReconnectPolicy::default(),
        };
        mqtt.validate()?;
        Ok(Self {
            mqtt,
            topics,
            installation_id: config.installation_id,
            node_id: config.node_id,
            instance_id: config.instance_id,
            inventory,
            store,
            status,
            artifacts,
            sequence,
        })
    }

    pub async fn run(self, cancel: CancellationToken) {
        self.status
            .update(|status| status.center_connection = CenterConnectionState::Connecting as i32);
        let mqtt = match MqttRuntime::new(
            self.mqtt.clone(),
            vec![MqttSubscription {
                topic_filter: self.topics.downstream_filter(),
                qos: MqttQos::AtLeastOnce,
            }],
        ) {
            Ok(mqtt) => mqtt,
            Err(error) => {
                base::log::error!("Steward MQTT configuration failed: {error}");
                self.status.update(|status| {
                    status.center_connection = CenterConnectionState::Degraded as i32
                });
                return;
            }
        };
        let publisher = mqtt.publisher();
        let (events_tx, events_rx) = mpsc::channel(128);
        let driver_cancel = cancel.child_token();
        let driver = base::tokio::spawn(mqtt.run(driver_cancel.clone(), events_tx));
        if let Err(error) = self.process(publisher, events_rx, cancel).await {
            base::log::error!("Steward MQTT processing failed: {error}");
            self.status
                .update(|status| status.center_connection = CenterConnectionState::Degraded as i32);
        }
        driver_cancel.cancel();
        match driver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => base::log::error!("Steward MQTT driver failed: {error}"),
            Err(error) => base::log::error!("Steward MQTT driver join failed: {error}"),
        }
    }

    async fn process(
        &self,
        publisher: MqttPublisher,
        mut events: mpsc::Receiver<MqttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut heartbeat = base::tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(base::tokio::time::MissedTickBehavior::Skip);
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = heartbeat.tick() => {
                    if self.status.snapshot().center_connection == CenterConnectionState::Connected as i32 {
                        self.publish_heartbeat(&publisher).await?;
                        self.publish_pending_receipts(&publisher).await?;
                    }
                }
                event = events.recv() => match event {
                    Some(MqttEvent::Connected) => {
                        self.status.update(|status| {
                            status.center_connection = CenterConnectionState::Connected as i32
                        });
                        self.publish_initial_state(&publisher).await?;
                    }
                    Some(MqttEvent::Disconnected(reason)) => {
                        base::log::warn!("Steward MQTT disconnected: {reason}");
                        self.status.update(|status| {
                            status.center_connection = CenterConnectionState::Degraded as i32
                        });
                    }
                    Some(MqttEvent::Publish { topic, payload, .. }) => {
                        self.handle_center_message(&publisher, &topic, &payload, cancel.child_token()).await?;
                    }
                    None => return Ok(()),
                }
            }
        }
    }

    async fn publish_initial_state(
        &self,
        publisher: &MqttPublisher,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.publish(
            publisher,
            self.topics.presence(),
            steward_to_center_message::Payload::Presence(StewardPresence {
                online: true,
                observed_at_epoch_ms: now_ms(),
            }),
            true,
            None,
        )
        .await?;
        self.publish(
            publisher,
            self.topics.hello(),
            steward_to_center_message::Payload::Hello(StewardHello {
                identity: Some(NodeIdentity {
                    node_id: self.node_id.clone(),
                    instance_id: self.instance_id.clone(),
                    kind: NodeKind::Steward as i32,
                }),
                software_version: env!("CARGO_PKG_VERSION").to_string(),
                manifest_revision: self.inventory.manifest_revision().to_string(),
                platform: std::env::consts::OS.to_string(),
                architecture: std::env::consts::ARCH.to_string(),
            }),
            false,
            None,
        )
        .await?;
        let inventory = self.inventory.refresh().await;
        self.store.save_inventory(&inventory).await?;
        self.publish(
            publisher,
            self.topics.inventory(),
            steward_to_center_message::Payload::Inventory(inventory),
            false,
            None,
        )
        .await?;
        self.publish_pending_receipts(publisher).await
    }

    async fn publish_heartbeat(
        &self,
        publisher: &MqttPublisher,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let snapshot = self.status.snapshot();
        self.publish(
            publisher,
            self.topics.heartbeat(),
            steward_to_center_message::Payload::Heartbeat(StewardHeartbeat {
                observed_at_epoch_ms: now_ms(),
                current_revision: snapshot.current_revision,
                desired_revision: snapshot.desired_revision,
                staged_revision: snapshot.staged_revision,
            }),
            false,
            None,
        )
        .await
    }

    async fn publish_pending_receipts(
        &self,
        publisher: &MqttPublisher,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for (message_id, receipt) in self.store.pending_receipts(128).await? {
            self.publish(
                publisher,
                self.topics.receipt(),
                steward_to_center_message::Payload::DeliveryReceipt(receipt),
                false,
                Some(message_id),
            )
            .await?;
        }
        Ok(())
    }

    async fn publish(
        &self,
        publisher: &MqttPublisher,
        topic: String,
        payload: steward_to_center_message::Payload,
        retain: bool,
        message_id: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let envelope = self.envelope(
            message_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
            payload,
        );
        publisher
            .publish(
                topic,
                envelope.encode_to_vec(),
                MqttQos::AtLeastOnce,
                retain,
            )
            .await?;
        Ok(())
    }

    async fn handle_center_message(
        &self,
        publisher: &MqttPublisher,
        topic: &str,
        payload: &[u8],
        cancel: CancellationToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let message = CenterToStewardMessage::decode(payload)?;
        validate_center_envelope(&message, &self.installation_id, &self.instance_id)?;
        match message.payload {
            Some(center_to_steward_message::Payload::ReceiptAck(ack))
                if topic == self.topics.receipt_ack() =>
            {
                self.store
                    .mark_receipt_sent(&ack.receipt_message_id, now_ms())
                    .await?;
            }
            Some(center_to_steward_message::Payload::DesiredState(desired))
                if topic == self.topics.desired() =>
            {
                self.handle_desired(publisher, desired, cancel).await?;
            }
            Some(center_to_steward_message::Payload::Command(_))
                if topic == self.topics.command() => {}
            Some(center_to_steward_message::Payload::UpgradeDecision(_))
                if topic == self.topics.upgrade_decision() => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "GMVC MQTT topic and protobuf payload mismatch",
                )
                .into());
            }
        }
        Ok(())
    }

    async fn handle_desired(
        &self,
        publisher: &MqttPublisher,
        desired: gmv_protocol::steward::v1::DesiredState,
        cancel: CancellationToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(artifact) = desired.artifact else {
            return Ok(());
        };
        let is_new = self
            .store
            .notice_delivery(
                &desired.assignment_id,
                &artifact.artifact_id,
                &artifact.revision,
                &artifact.sha256,
                now_ms(),
            )
            .await?;
        self.status.update(|status| {
            status.desired_revision = artifact.revision.clone();
            status.delivery_state = "NOTICED".to_string();
        });
        if !is_new {
            self.publish_pending_receipts(publisher).await?;
            return Ok(());
        }
        self.status
            .update(|status| status.delivery_state = "DOWNLOADING".to_string());
        let (state, stable_error_code) = if desired.deadline_epoch_ms <= now_ms() {
            (DeliveryState::Failed, "desired_expired".to_string())
        } else {
            match self.artifacts.stage(&artifact, cancel).await {
                Ok(_) => {
                    self.status.update(|status| {
                        status.staged_revision = artifact.revision.clone();
                        status.delivery_state = "STAGED".to_string();
                    });
                    (DeliveryState::Staged, String::new())
                }
                Err(error) => (DeliveryState::Failed, stable_error_code(&error).to_string()),
            }
        };
        if state == DeliveryState::Failed {
            self.status
                .update(|status| status.delivery_state = "FAILED".to_string());
        }
        let receipt = DeliveryReceipt {
            assignment_id: desired.assignment_id,
            artifact_id: artifact.artifact_id,
            revision: artifact.revision,
            state: state as i32,
            stable_error_code,
            observed_at_epoch_ms: now_ms(),
            ..DeliveryReceipt::default()
        };
        let receipt_id = uuid::Uuid::now_v7().to_string();
        self.store.record_receipt(&receipt_id, &receipt).await?;
        self.publish(
            publisher,
            self.topics.receipt(),
            steward_to_center_message::Payload::DeliveryReceipt(receipt),
            false,
            Some(receipt_id),
        )
        .await
    }

    fn envelope(
        &self,
        message_id: String,
        payload: steward_to_center_message::Payload,
    ) -> StewardToCenterMessage {
        StewardToCenterMessage {
            message_id,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            sent_at_epoch_ms: now_ms(),
            installation_id: self.installation_id.clone(),
            steward_instance_id: self.instance_id.clone(),
            protocol_version: 1,
            payload: Some(payload),
        }
    }
}

fn validate_center_envelope(
    message: &CenterToStewardMessage,
    installation_id: &str,
    instance_id: &str,
) -> Result<(), io::Error> {
    if message.protocol_version != 1
        || message.message_id.is_empty()
        || message.sequence == 0
        || message.payload.is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid GMVC envelope",
        ));
    }
    if message.installation_id != installation_id
        || (!message.expected_steward_instance_id.is_empty()
            && message.expected_steward_instance_id != instance_id)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "GMVC envelope target mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gmv_protocol::steward::v1::DesiredState;

    #[test]
    fn retained_desired_can_target_the_installation_across_steward_restarts() {
        let message = CenterToStewardMessage {
            message_id: "m1".to_string(),
            sequence: 1,
            installation_id: "site-1".to_string(),
            expected_steward_instance_id: String::new(),
            protocol_version: 1,
            payload: Some(center_to_steward_message::Payload::DesiredState(
                DesiredState::default(),
            )),
            ..CenterToStewardMessage::default()
        };
        assert!(validate_center_envelope(&message, "site-1", "instance-2").is_ok());
    }

    #[test]
    fn rejects_message_for_another_active_instance() {
        let message = CenterToStewardMessage {
            message_id: "m1".to_string(),
            sequence: 1,
            installation_id: "site-1".to_string(),
            expected_steward_instance_id: "instance-1".to_string(),
            protocol_version: 1,
            payload: Some(center_to_steward_message::Payload::DesiredState(
                DesiredState::default(),
            )),
            ..CenterToStewardMessage::default()
        };
        assert!(validate_center_envelope(&message, "site-1", "instance-2").is_err());
    }
}
