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
use base_rpc::{
    ConnectionReporter, RpcChannelConfig, StreamConnector, StreamSupervisor,
    StreamSupervisorConfig, connect_channel,
};
use gmv_protocol::{
    common::v1::{NodeIdentity, NodeKind},
    guard::v1::CenterConnectionState,
    steward::v1::{
        DeliveryReceipt, DeliveryState, StewardHeartbeat, StewardHello, StewardToCenterMessage,
        center_to_steward_message, steward_gateway_client::StewardGatewayClient,
        steward_to_center_message,
    },
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::async_trait;

#[derive(Clone)]
pub struct CenterConnector {
    channel: RpcChannelConfig,
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

struct ConnectionStatusGuard {
    status: SharedStatus,
    cancel: CancellationToken,
}

impl Drop for ConnectionStatusGuard {
    fn drop(&mut self) {
        if !self.cancel.is_cancelled() {
            self.status
                .update(|status| status.center_connection = CenterConnectionState::Degraded as i32);
        }
    }
}

impl CenterConnector {
    pub fn new(
        config: CenterConnectorConfig,
        inventory: InventoryService,
        store: StateStore,
        status: SharedStatus,
        artifacts: ArtifactService,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut channel = RpcChannelConfig::new(config.center.endpoint.clone());
        if let Some(tls) = config.center.tls {
            channel.tls = Some(base_rpc::load_client_tls_from_files(
                &base_rpc::TlsFileConfig {
                    ca_certificate_path: Some(tls.ca_certificate_path),
                    client_certificate_path: Some(tls.client_certificate_path),
                    client_private_key_path: Some(tls.client_private_key_path),
                    domain_name: tls.domain_name,
                    ..base_rpc::TlsFileConfig::default()
                },
            )?);
        }
        Ok(Self {
            channel,
            installation_id: config.installation_id,
            node_id: config.node_id,
            instance_id: config.instance_id,
            inventory,
            store,
            status,
            artifacts,
            sequence: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn run(self, cancel: CancellationToken) {
        self.status
            .update(|status| status.center_connection = CenterConnectionState::Connecting as i32);
        let handle = StreamSupervisor::new(StreamSupervisorConfig::default(), self).spawn();
        cancel.cancelled().await;
        handle.shutdown().await;
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

#[async_trait]
impl StreamConnector for CenterConnector {
    type Error = io::Error;

    async fn connect_and_run(
        &self,
        _generation: u64,
        reporter: ConnectionReporter,
        cancel: CancellationToken,
    ) -> Result<(), Self::Error> {
        self.status
            .update(|status| status.center_connection = CenterConnectionState::Connecting as i32);
        let _connection_status = ConnectionStatusGuard {
            status: self.status.clone(),
            cancel: cancel.clone(),
        };
        let channel = connect_channel(&self.channel).await.map_err(io_other)?;
        let mut client = StewardGatewayClient::new(channel);
        let (tx, rx) = mpsc::channel(32);
        let mut inbound = client
            .open_control_stream(ReceiverStream::new(rx))
            .await
            .map_err(io_other)?
            .into_inner();
        reporter.connected();
        self.status
            .update(|status| status.center_connection = CenterConnectionState::Connected as i32);

        let hello = StewardHello {
            identity: Some(NodeIdentity {
                node_id: self.node_id.clone(),
                instance_id: self.instance_id.clone(),
                kind: NodeKind::Steward as i32,
            }),
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            manifest_revision: self.inventory.manifest_revision().to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
        };
        tx.send(self.envelope(
            uuid::Uuid::now_v7().to_string(),
            steward_to_center_message::Payload::Hello(hello),
        ))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "GMVC stream closed"))?;

        let inventory = self.inventory.refresh().await;
        self.store
            .save_inventory(&inventory)
            .await
            .map_err(io_other)?;
        tx.send(self.envelope(
            uuid::Uuid::now_v7().to_string(),
            steward_to_center_message::Payload::Inventory(inventory),
        ))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "GMVC stream closed"))?;
        for (message_id, receipt) in self.store.pending_receipts(128).await.map_err(io_other)? {
            tx.send(self.envelope(
                message_id.clone(),
                steward_to_center_message::Payload::DeliveryReceipt(receipt),
            ))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "GMVC stream closed"))?;
        }

        let mut heartbeat = base::tokio::time::interval(Duration::from_secs(15));
        let mut last_center_sequence = 0;
        loop {
            base::tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = heartbeat.tick() => {
                    let snapshot = self.status.snapshot();
                    let heartbeat = StewardHeartbeat {
                        observed_at_epoch_ms: now_ms(),
                        current_revision: snapshot.current_revision,
                        desired_revision: snapshot.desired_revision,
                        staged_revision: snapshot.staged_revision,
                    };
                    tx.send(self.envelope(
                        uuid::Uuid::now_v7().to_string(),
                        steward_to_center_message::Payload::Heartbeat(heartbeat),
                    ))
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "GMVC stream closed"))?;
                }
                message = inbound.message() => {
                    let message = message.map_err(io_other)?.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "GMVC stream ended")
                    })?;
                    validate_center_envelope(
                        &message,
                        &self.installation_id,
                        &self.instance_id,
                        last_center_sequence,
                    )?;
                    last_center_sequence = message.sequence;
                    match message.payload {
                        Some(center_to_steward_message::Payload::ReceiptAck(ack)) => {
                            self.store
                                .mark_receipt_sent(&ack.receipt_message_id, now_ms())
                                .await
                                .map_err(io_other)?;
                        }
                        Some(center_to_steward_message::Payload::DesiredState(desired)) => {
                            let Some(artifact) = desired.artifact else { continue };
                            let is_new = self.store.notice_delivery(
                                &desired.assignment_id,
                                &artifact.artifact_id,
                                &artifact.revision,
                                &artifact.sha256,
                                now_ms(),
                            ).await.map_err(io_other)?;
                            self.status.update(|status| {
                                status.desired_revision = artifact.revision.clone();
                                status.delivery_state = "NOTICED".to_string();
                            });
                            if is_new {
                                self.status.update(|status| {
                                    status.delivery_state = "DOWNLOADING".to_string();
                                });
                                let (state, stable_error_code) = if desired.deadline_epoch_ms <= now_ms() {
                                    self.status.update(|status| {
                                        status.delivery_state = "FAILED".to_string();
                                    });
                                    (DeliveryState::Failed, "desired_expired".to_string())
                                } else {
                                    let staged = self.artifacts.stage(&artifact, cancel.child_token()).await;
                                    match staged {
                                        Ok(_) => {
                                            self.status.update(|status| {
                                                status.staged_revision = artifact.revision.clone();
                                                status.delivery_state = "STAGED".to_string();
                                            });
                                            (DeliveryState::Staged, String::new())
                                        }
                                        Err(error) => {
                                            let code = stable_error_code(&error).to_string();
                                            self.status.update(|status| {
                                                status.delivery_state = "FAILED".to_string();
                                            });
                                            (DeliveryState::Failed, code)
                                        }
                                    }
                                };
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
                                self.store.record_receipt(&receipt_id, &receipt).await.map_err(io_other)?;
                                tx.send(self.envelope(
                                    receipt_id,
                                    steward_to_center_message::Payload::DeliveryReceipt(receipt),
                                )).await.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "GMVC stream closed"))?;
                            }
                        }
                        Some(center_to_steward_message::Payload::Command(_)) | None => {}
                    }
                }
            }
        }
    }
}

fn validate_center_envelope(
    message: &gmv_protocol::steward::v1::CenterToStewardMessage,
    installation_id: &str,
    instance_id: &str,
    last_sequence: u64,
) -> Result<(), io::Error> {
    if message.protocol_version != 1
        || message.message_id.is_empty()
        || message.sequence == 0
        || message.sequence <= last_sequence
        || message.payload.is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid GMVC envelope",
        ));
    }
    if message.installation_id != installation_id
        || message.expected_steward_instance_id != instance_id
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "GMVC envelope target mismatch",
        ));
    }
    Ok(())
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact::ArtifactService,
        config::{ComponentSpec, InstallManifest, StewardConfig, TrustedSigningKey},
        inventory::{InventoryAdapter, ServiceObservation},
    };
    use base::{
        base64::Engine,
        sha2::{Digest, Sha256},
    };
    use ed25519_dalek::{Signer, SigningKey};
    use gmv_protocol::steward::v1::steward_gateway_server::{StewardGateway, StewardGatewayServer};
    use gmv_protocol::steward::v1::{
        ArtifactManifest, CenterToStewardMessage, DesiredState, ReceiptAck,
        center_to_steward_message, steward_to_center_message,
    };
    use std::{path::PathBuf, pin::Pin};
    use tokio_stream::{Stream, wrappers::ReceiverStream};
    use tonic::{Request, Response, Status};

    fn center_message(sequence: u64, installation_id: &str) -> CenterToStewardMessage {
        CenterToStewardMessage {
            message_id: format!("message-{sequence}"),
            sequence,
            sent_at_epoch_ms: 1,
            installation_id: installation_id.to_string(),
            expected_steward_instance_id: "instance-1".to_string(),
            protocol_version: 1,
            payload: Some(center_to_steward_message::Payload::ReceiptAck(ReceiptAck {
                receipt_message_id: "receipt-1".to_string(),
            })),
        }
    }

    #[test]
    fn center_envelope_is_targeted_and_monotonic() {
        let message = center_message(2, "installation-1");
        assert!(validate_center_envelope(&message, "installation-1", "instance-1", 1).is_ok());
        assert_eq!(
            validate_center_envelope(&message, "installation-1", "instance-1", 2)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            validate_center_envelope(&message, "installation-2", "instance-1", 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    struct FakeInventory;

    #[tonic::async_trait]
    impl InventoryAdapter for FakeInventory {
        async fn observe(
            &self,
            _unit: &str,
        ) -> Result<ServiceObservation, Box<dyn std::error::Error + Send + Sync>> {
            Ok(ServiceObservation {
                loaded: true,
                active: true,
                version: Some("1.0.0".to_string()),
            })
        }
    }

    #[derive(Clone)]
    struct FakeGateway {
        desired: DesiredState,
        receipt_tx: base::tokio::sync::mpsc::Sender<DeliveryReceipt>,
    }

    type GatewayStream =
        Pin<Box<dyn Stream<Item = Result<CenterToStewardMessage, Status>> + Send + 'static>>;

    #[tonic::async_trait]
    impl StewardGateway for FakeGateway {
        type OpenControlStreamStream = GatewayStream;

        async fn open_control_stream(
            &self,
            request: Request<tonic::Streaming<StewardToCenterMessage>>,
        ) -> Result<Response<Self::OpenControlStreamStream>, Status> {
            let mut inbound = request.into_inner();
            let desired = self.desired.clone();
            let receipt_tx = self.receipt_tx.clone();
            let (outbound_tx, outbound_rx) = base::tokio::sync::mpsc::channel(4);
            base::tokio::spawn(async move {
                let mut desired_sent = false;
                while let Ok(Some(message)) = inbound.message().await {
                    match message.payload {
                        Some(steward_to_center_message::Payload::Hello(_)) if !desired_sent => {
                            desired_sent = true;
                            let response = CenterToStewardMessage {
                                message_id: "desired-message".to_string(),
                                sequence: 1,
                                sent_at_epoch_ms: crate::now_ms(),
                                installation_id: message.installation_id,
                                expected_steward_instance_id: message.steward_instance_id,
                                protocol_version: 1,
                                payload: Some(center_to_steward_message::Payload::DesiredState(
                                    desired.clone(),
                                )),
                            };
                            if outbound_tx.send(Ok(response)).await.is_err() {
                                break;
                            }
                        }
                        Some(steward_to_center_message::Payload::DeliveryReceipt(receipt)) => {
                            let response = CenterToStewardMessage {
                                message_id: "receipt-ack".to_string(),
                                sequence: 2,
                                sent_at_epoch_ms: crate::now_ms(),
                                installation_id: message.installation_id,
                                expected_steward_instance_id: message.steward_instance_id,
                                protocol_version: 1,
                                payload: Some(center_to_steward_message::Payload::ReceiptAck(
                                    ReceiptAck {
                                        receipt_message_id: message.message_id,
                                    },
                                )),
                            };
                            let _ = outbound_tx.send(Ok(response)).await;
                            let _ = receipt_tx.send(receipt).await;
                            break;
                        }
                        _ => {}
                    }
                }
            });
            Ok(Response::new(Box::pin(ReceiverStream::new(outbound_rx))))
        }
    }

    async fn serve_artifact(body: Vec<u8>) -> String {
        use base::tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = base::tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        base::tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
        });
        format!("http://{address}/bundle")
    }

    fn artifact_service(root: PathBuf, signing_key: &SigningKey) -> ArtifactService {
        ArtifactService::from_config(&StewardConfig {
            installation_id: "installation-1".to_string(),
            node_id: "steward-1".to_string(),
            manifest_path: root.join("manifest.yml"),
            database_path: root.join("state.db"),
            staging_root: root.join("staging"),
            max_artifact_bytes: 1024,
            artifact_timeout_secs: 5,
            artifact_allowed_hosts: vec!["127.0.0.1".to_string()],
            trusted_signing_keys: vec![TrustedSigningKey {
                key_id: "test-key".to_string(),
                public_key_base64: base::base64::engine::general_purpose::STANDARD
                    .encode(signing_key.verifying_key().as_bytes()),
            }],
            guard_endpoint: None,
            gmvc: Some(CenterConfig {
                endpoint: "http://127.0.0.1:1".to_string(),
                allow_plaintext: true,
                tls: None,
            }),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn desired_bundle_reaches_staged_receipt_and_durable_ack() {
        let root = std::env::temp_dir().join(format!("steward-e2e-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let body = b"vertical slice bundle".to_vec();
        let signing_key = SigningKey::from_bytes(&[9; 32]);
        let artifact = ArtifactManifest {
            artifact_id: "gmv".to_string(),
            version: "1.0.0".to_string(),
            revision: "revision-1".to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            content_size: body.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&body)),
            signature: signing_key.sign(&body).to_bytes().to_vec(),
            signing_key_id: "test-key".to_string(),
            download_url: serve_artifact(body.clone()).await,
            download_expires_at_epoch_ms: crate::now_ms() + 30_000,
            ..ArtifactManifest::default()
        };
        let desired = DesiredState {
            assignment_id: "assignment-1".to_string(),
            channel: "test".to_string(),
            artifact: Some(artifact),
            deadline_epoch_ms: crate::now_ms() + 30_000,
        };
        let (receipt_tx, mut receipt_rx) = base::tokio::sync::mpsc::channel(1);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let incoming = base_rpc::tcp_incoming_from_std(listener).unwrap();
        let server_cancel = CancellationToken::new();
        let shutdown = server_cancel.clone();
        let server = base::tokio::spawn(async move {
            base_rpc::build_server(&base_rpc::RpcServerConfig::default())
                .unwrap()
                .add_service(StewardGatewayServer::new(FakeGateway {
                    desired,
                    receipt_tx,
                }))
                .serve_with_incoming_shutdown(incoming, async move {
                    shutdown.cancelled().await;
                })
                .await
                .unwrap();
        });

        let store = StateStore::open(&root.join("state.db")).await.unwrap();
        let inventory = InventoryService::new(
            InstallManifest {
                revision: "manifest-1".to_string(),
                components: vec![ComponentSpec {
                    component_id: "avai".to_string(),
                    service_type: "avai".to_string(),
                    systemd_unit: "gmv-avai.service".to_string(),
                    version: "1.0.0".to_string(),
                }],
            },
            Arc::new(FakeInventory),
        );
        let status = SharedStatus::new("installation-1".to_string(), "instance-1".to_string());
        let connector = CenterConnector::new(
            CenterConnectorConfig {
                center: CenterConfig {
                    endpoint: format!("http://{address}"),
                    allow_plaintext: true,
                    tls: None,
                },
                installation_id: "installation-1".to_string(),
                node_id: "steward-1".to_string(),
                instance_id: "instance-1".to_string(),
            },
            inventory,
            store.clone(),
            status.clone(),
            artifact_service(root.clone(), &signing_key),
        )
        .unwrap();
        let connector_cancel = CancellationToken::new();
        let connector_shutdown = connector_cancel.clone();
        let connector_task = base::tokio::spawn(connector.run(connector_shutdown));

        let receipt = base::tokio::time::timeout(Duration::from_secs(5), receipt_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receipt.state, DeliveryState::Staged as i32);
        assert_eq!(status.snapshot().staged_revision, "revision-1");
        base::tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if store.pending_receipts(10).await.unwrap().is_empty() {
                    break;
                }
                base::tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            base::tokio::fs::read(root.join("staging/gmv-revision-1.bundle"))
                .await
                .unwrap(),
            body
        );

        connector_cancel.cancel();
        connector_task.await.unwrap();
        server_cancel.cancel();
        server.await.unwrap();
        store.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
