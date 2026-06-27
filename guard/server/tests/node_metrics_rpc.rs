use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base::tokio_util::sync::CancellationToken;
use base_rpc::RpcChannelConfig;
use gmv_nodec::{NodeReporter, NodeReporterConfig};
use gmv_protocol::common::v1::{NodeIdentity, NodeKind, ResourceRef};
use gmv_protocol::guard::v1::guard_node_control_client::GuardNodeControlClient;
use gmv_protocol::guard::v1::guard_node_control_server::{
    GuardNodeControl, GuardNodeControlServer,
};
use gmv_protocol::guard::v1::{
    EventPriority, NodeEvent, NodeResourceSnapshot, NodeToGuardMessage, RegisterNodeRequest,
    ResourceReport, ResourceState, node_to_guard_message,
};
use guard::registry::RegistryService;
use guard::runtime::node_rpc::GuardNodeRpc;
use guard::store::InMemoryGuardStore;

#[test]
fn node_reporter_registers_and_updates_host_metrics_over_grpc() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let store = InMemoryGuardStore::default();
            let service = GuardNodeRpc::new(
                RegistryService::new(store.clone()),
                store.clone(),
                100,
                1_000,
                None,
            );
            let server_cancel = CancellationToken::new();
            let server_shutdown = server_cancel.clone();
            let server = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(GuardNodeControlServer::new(service))
                    .serve_with_shutdown(address, async move { server_shutdown.cancelled().await })
                    .await
                    .unwrap();
            });

            let register = RegisterNodeRequest {
                identity: Some(NodeIdentity {
                    node_id: "stream-test".to_string(),
                    instance_id: "instance-test".to_string(),
                    kind: NodeKind::Stream as i32,
                }),
                software_version: "test".to_string(),
                started_at_epoch_ms: 1,
                endpoints: vec![],
                capabilities: vec!["live".to_string()],
                startup_snapshot: Some(NodeResourceSnapshot::default()),
                host_metrics: None,
                capacity: 10,
                zone: "test".to_string(),
                takeover: false,
            };
            let mut config = NodeReporterConfig::new(
                RpcChannelConfig::new(format!("http://{address}")),
                register,
            );
            config.reconnect_delay = Duration::from_millis(20);
            config.business_metrics =
                Arc::new(|| HashMap::from([("receiving_streams".to_string(), "3".to_string())]));
            let reporter_cancel = CancellationToken::new();
            let reporter = NodeReporter::spawn(config, reporter_cancel.clone());

            base::tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(node) = store.get_node("stream-test") {
                        if node.sequence > 0 && node.host_metrics.memory_total_bytes > 0 {
                            assert_eq!(
                                node.business_metrics
                                    .get("receiving_streams")
                                    .map(String::as_str),
                                Some("3")
                            );
                            break;
                        }
                    }
                    base::tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();

            reporter_cancel.cancel();
            reporter.await.unwrap();
            server_cancel.cancel();
            server.await.unwrap();
        });
}

#[test]
fn register_consumes_startup_snapshot() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let store = InMemoryGuardStore::default();
            let service = GuardNodeRpc::new(
                RegistryService::new(store.clone()),
                store.clone(),
                100,
                1_000,
                None,
            );
            GuardNodeControl::register_node(
                &service,
                tonic::Request::new(RegisterNodeRequest {
                    identity: Some(NodeIdentity {
                        node_id: "stream-snapshot".to_string(),
                        instance_id: "instance-snapshot".to_string(),
                        kind: NodeKind::Stream as i32,
                    }),
                    software_version: "test".to_string(),
                    started_at_epoch_ms: 1,
                    endpoints: vec![],
                    capabilities: vec!["live".to_string()],
                    startup_snapshot: Some(snapshot("stream-1", "route-1")),
                    host_metrics: None,
                    capacity: 10,
                    zone: String::new(),
                    takeover: false,
                }),
            )
            .await
            .unwrap();

            let route = store.get_route("route-1").unwrap();
            assert_eq!(route.resource_id, "stream-1");
            assert_eq!(route.node_id, "stream-snapshot");
        });
}

#[test]
fn control_stream_consumes_snapshot_and_event_payloads() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let store = InMemoryGuardStore::default();
            let service = GuardNodeRpc::new(
                RegistryService::new(store.clone()),
                store.clone(),
                100,
                1_000,
                None,
            );
            let server_cancel = CancellationToken::new();
            let server_shutdown = server_cancel.clone();
            let server = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(GuardNodeControlServer::new(service))
                    .serve_with_shutdown(address, async move { server_shutdown.cancelled().await })
                    .await
                    .unwrap();
            });

            let mut client = connect_client(address).await;
            let identity = NodeIdentity {
                node_id: "stream-control".to_string(),
                instance_id: "instance-control".to_string(),
                kind: NodeKind::Stream as i32,
            };
            client
                .register_node(RegisterNodeRequest {
                    identity: Some(identity.clone()),
                    software_version: "test".to_string(),
                    started_at_epoch_ms: 1,
                    endpoints: vec![],
                    capabilities: vec!["live".to_string()],
                    startup_snapshot: Some(NodeResourceSnapshot::default()),
                    host_metrics: None,
                    capacity: 10,
                    zone: String::new(),
                    takeover: false,
                })
                .await
                .unwrap();

            let (tx, rx) = base::tokio::sync::mpsc::channel(4);
            let mut output = client
                .open_control_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
                .await
                .unwrap()
                .into_inner();
            tx.send(NodeToGuardMessage {
                identity: Some(identity.clone()),
                sequence: 1,
                sent_at_epoch_ms: 1,
                payload: Some(node_to_guard_message::Payload::Snapshot(snapshot(
                    "stream-2", "route-2",
                ))),
            })
            .await
            .unwrap();
            output.message().await.unwrap().unwrap();
            tx.send(NodeToGuardMessage {
                identity: Some(identity),
                sequence: 2,
                sent_at_epoch_ms: 2,
                payload: Some(node_to_guard_message::Payload::Event(NodeEvent {
                    event_id: "evt-node-1".to_string(),
                    topic: "stream.frame.ready".to_string(),
                    priority: EventPriority::P2 as i32,
                    payload: b"ok".to_vec(),
                })),
            })
            .await
            .unwrap();
            output.message().await.unwrap().unwrap();

            assert_eq!(store.get_route("route-2").unwrap().resource_id, "stream-2");
            let event = store.events_after(None, 10).pop().unwrap();
            assert_eq!(event.event_id, "evt-node-1");
            assert_eq!(event.topic, "stream.frame.ready");

            drop(tx);
            server_cancel.cancel();
            server.await.unwrap();
        });
}

async fn connect_client(
    address: std::net::SocketAddr,
) -> GuardNodeControlClient<tonic::transport::Channel> {
    let endpoint = format!("http://{address}");
    let mut last_error = None;
    for _ in 0..50 {
        match GuardNodeControlClient::connect(endpoint.clone()).await {
            Ok(client) => return client,
            Err(error) => {
                last_error = Some(error);
                base::tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    panic!("failed to connect test guard node rpc: {:?}", last_error);
}

fn snapshot(resource_id: &str, route_id: &str) -> NodeResourceSnapshot {
    NodeResourceSnapshot {
        full: true,
        resources: vec![ResourceReport {
            resource: Some(ResourceRef {
                resource_id: resource_id.to_string(),
                resource_type: "stream".to_string(),
            }),
            state: ResourceState::Running as i32,
            labels: HashMap::from([("route_id".to_string(), route_id.to_string())]),
        }],
    }
}
