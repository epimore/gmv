use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};

use gmv_protocol::avai::v1::avai_control_server::{AvaiControl, AvaiControlServer};
use gmv_protocol::avai::v1::{
    AiTaskState, CancelTaskRequest, CancelTaskResponse, CreateTaskRequest, CreateTaskResponse,
    QueryCapabilitiesRequest, QueryCapabilitiesResponse, QueryTaskRequest, QueryTaskResponse,
};
use gmv_protocol::common::v1::PageResponse;
use gmv_protocol::session::v1::session_control_server::{SessionControl, SessionControlServer};
use gmv_protocol::session::v1::{
    ControlPtzRequest, ControlPtzResponse, CreateGbDeviceRequest, CreateGbDeviceResponse,
    DeviceStreamResponse, DeviceStreamState, GbDevice, GetGbChannelRequest, GetGbChannelResponse,
    GetGbDeviceRequest, GetGbDeviceResponse, GetSessionConfigRequest, GetSessionConfigResponse,
    ListGbChannelImagesRequest, ListGbChannelImagesResponse, ListGbChannelsRequest,
    ListGbChannelsResponse, ListGbDevicesRequest, ListGbDevicesResponse, SnapshotImageRequest,
    SnapshotImageResponse, StartDeviceStreamRequest, StopDeviceStreamRequest,
};
use gmv_protocol::stream::v1::stream_control_server::{StreamControl, StreamControlServer};
use gmv_protocol::stream::v1::{
    CloseOutputRequest, CloseOutputResponse, CreateOutputRequest, CreateOutputResponse,
    GetPlaybackEndpointsRequest, GetPlaybackEndpointsResponse, QueryStreamRequest,
    QueryStreamResponse, StartReceiveRequest, StartReceiveResponse, StopReceiveRequest,
    StopReceiveResponse, StreamBoolResponse, StreamJsonRequest, StreamJsonResponse, StreamState,
    StreamUnitResponse,
};
use guard::api::v2::control::BusinessControl;
use guard::core::{ConnectionState, HealthState, NodeIdentity, NodeKind, SchedulingState};
use guard::mqttc::{CommandAction, MqttCommandExecutor, RoutedCommand};
use guard::operation::{OperationService, OperationStatus};
use guard::registry::{RegisterRequest, RegistryService};
use guard::store::InMemoryGuardStore;
use guard::store::model::{EndpointModeRecord, EndpointRecord, HostMetricsRecord, NodeRecord};

#[test]
fn gb28181_create_device_uses_selected_session_rpc() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let session_addr = free_loopback_addr();
            let _session = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(SessionControlServer::new(FakeSession))
                    .serve(session_addr)
                    .await
                    .unwrap();
            });
            base::tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let store = InMemoryGuardStore::default();
            let registry = RegistryService::new(store.clone());
            registry
                .register(RegisterRequest {
                    identity: NodeIdentity::new(
                        "session-gb-online",
                        "session-inst",
                        NodeKind::Session,
                    ),
                    capabilities: vec!["protocol.gb28181".to_string()],
                    endpoints: vec![grpc_endpoint(session_addr)],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: HashMap::from([
                        ("service".to_string(), "session-gb28181".to_string()),
                        ("protocol".to_string(), "gb28181".to_string()),
                    ]),
                })
                .unwrap();

            let device = BusinessControl::new(store)
                .create_gb_device(gmv_protocol::session::v1::GbDevice {
                    session_node_id: "session-gb-online".to_string(),
                    domain_id: "34020000002000000001".to_string(),
                    domain: "3402000000".to_string(),
                    alias: "front door".to_string(),
                    status: 1,
                    heartbeat_sec: 60,
                    ..Default::default()
                })
                .await
                .unwrap();
            assert_eq!(device.session_node_id, "session-gb-online");
            assert_eq!(device.device_id, "34020000001320000001");
            assert_eq!(device.alias, "front door");
        });
}

#[test]
fn gb28181_session_node_config_uses_rpc_and_skips_offline_nodes() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let session_addr = free_loopback_addr();
            let _session = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(SessionControlServer::new(FakeSession))
                    .serve(session_addr)
                    .await
                    .unwrap();
            });
            base::tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let store = InMemoryGuardStore::default();
            let registry = RegistryService::new(store.clone());
            registry
                .register(RegisterRequest {
                    identity: NodeIdentity::new(
                        "session-gb-online",
                        "session-inst",
                        NodeKind::Session,
                    ),
                    capabilities: vec!["protocol.gb28181".to_string()],
                    endpoints: vec![grpc_endpoint(session_addr)],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: HashMap::from([
                        ("service".to_string(), "session-gb28181".to_string()),
                        ("protocol".to_string(), "gb28181".to_string()),
                    ]),
                })
                .unwrap();
            store.upsert_node(NodeRecord {
                identity: NodeIdentity::new("session-gb-offline", "offline", NodeKind::Session),
                connection: ConnectionState::Disconnected,
                health: HealthState::Offline,
                scheduling: SchedulingState::Disabled,
                endpoints: vec![],
                capabilities: vec!["protocol.gb28181".to_string()],
                capacity: 0,
                pending_leases: 0,
                host_metrics: HostMetricsRecord::default(),
                business_metrics: HashMap::new(),
                config: HashMap::from([
                    ("service".to_string(), "session-gb28181".to_string()),
                    ("protocol".to_string(), "gb28181".to_string()),
                ]),
                zone: None,
                last_seen_at_ms: 0,
                generation: 0,
                sequence: 0,
            });

            let config = BusinessControl::new(store.clone())
                .gb_session_config("session-gb-online")
                .await
                .unwrap();
            assert_eq!(config.domain, "3402000000");
            assert_eq!(config.domain_id, "34020000002000000001");
            assert_eq!(config.wan_ip, "101.33.200.169");
            assert_eq!(config.wan_port, 25600);

            let error = BusinessControl::new(store)
                .gb_session_config("session-gb-offline")
                .await
                .unwrap_err();
            assert!(error.to_string().contains("offline"));
        });
}
#[test]
fn gb28181_snapshot_image_uses_session_rpc() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let session_addr = free_loopback_addr();
            let _session = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(SessionControlServer::new(FakeSession))
                    .serve(session_addr)
                    .await
                    .unwrap();
            });
            base::tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let store = InMemoryGuardStore::default();
            RegistryService::new(store.clone())
                .register(RegisterRequest {
                    identity: NodeIdentity::new(
                        "session-gb-online",
                        "session-inst",
                        NodeKind::Session,
                    ),
                    capabilities: vec!["protocol.gb28181".to_string()],
                    endpoints: vec![grpc_endpoint(session_addr)],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: HashMap::from([
                        ("service".to_string(), "session-gb28181".to_string()),
                        ("protocol".to_string(), "gb28181".to_string()),
                    ]),
                })
                .unwrap();

            let session_id = BusinessControl::new(store)
                .snapshot_image(
                    "snapshot-op",
                    "34020000001320000001",
                    "34020000001320000002",
                    1,
                    1,
                )
                .await
                .unwrap();
            assert_eq!(session_id, "snapshot-session");
        });
}

#[test]
fn guard_business_control_uses_registered_rpc_endpoints_for_live_ptz_and_stop() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let session_addr = free_loopback_addr();
            let stream_addr = free_loopback_addr();
            let avai_addr = free_loopback_addr();
            let _session = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(SessionControlServer::new(FakeSession))
                    .serve(session_addr)
                    .await
                    .unwrap();
            });
            let _stream = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(StreamControlServer::new(FakeStream))
                    .serve(stream_addr)
                    .await
                    .unwrap();
            });
            let _avai = base::tokio::spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(AvaiControlServer::new(FakeAvai))
                    .serve(avai_addr)
                    .await
                    .unwrap();
            });
            base::tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let store = InMemoryGuardStore::default();
            let registry = RegistryService::new(store.clone());
            registry
                .register(RegisterRequest {
                    identity: NodeIdentity::new("session-rpc", "session-inst", NodeKind::Session),
                    capabilities: vec![
                        "device.live".to_string(),
                        "device.playback".to_string(),
                        "device.download".to_string(),
                        "device.talk".to_string(),
                        "device.ptz".to_string(),
                    ],
                    endpoints: vec![grpc_endpoint(session_addr)],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: Default::default(),
                })
                .unwrap();
            registry
                .register(RegisterRequest {
                    identity: NodeIdentity::new("stream-rpc", "stream-inst", NodeKind::Stream),
                    capabilities: vec![
                        "live".to_string(),
                        "playback".to_string(),
                        "download".to_string(),
                        "talk".to_string(),
                    ],
                    endpoints: vec![
                        grpc_endpoint(stream_addr),
                        EndpointRecord {
                            name: "rtp".to_string(),
                            scheme: "rtp".to_string(),
                            host: "127.0.0.1".to_string(),
                            port: 30000,
                            mode: EndpointModeRecord::Single,
                            labels: HashMap::new(),
                        },
                    ],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: Default::default(),
                })
                .unwrap();
            registry
                .register(RegisterRequest {
                    identity: NodeIdentity::new("avai-rpc", "avai-inst", NodeKind::Avai),
                    capabilities: vec!["ai.vehicle".to_string()],
                    endpoints: vec![grpc_endpoint(avai_addr)],
                    capacity: 8,
                    host_metrics: Default::default(),
                    zone: None,
                    now_ms: 1_000,
                    takeover: false,
                    config: Default::default(),
                })
                .unwrap();

            let control = BusinessControl::new(store.clone());
            let stream = control
                .start_live("op-live-rpc", "device-1", "channel-1")
                .await
                .unwrap();
            assert_eq!(stream.stream_id, "live-op-live-rpc");
            assert_eq!(stream.node_id, "session-rpc");
            assert_eq!(stream.endpoint, "rtp://127.0.0.1:30000/live-op-live-rpc");

            assert_eq!(
                control
                    .start_playback("op-playback-rpc", "device-1", "channel-1")
                    .await
                    .unwrap()
                    .stream_id,
                "playback-op-playback-rpc"
            );
            assert_eq!(
                control
                    .start_download("op-download-rpc", "device-1", "channel-1")
                    .await
                    .unwrap()
                    .stream_id,
                "download-op-download-rpc"
            );
            assert_eq!(
                control
                    .start_talk("op-talk-rpc", "device-1", "channel-1")
                    .await
                    .unwrap()
                    .stream_id,
                "talk-op-talk-rpc"
            );
            assert_eq!(control.ptz("device-1", "channel-1").await.unwrap(), 1);
            let ai_task = control
                .start_ai("op-ai-rpc", &stream.stream_id, "vehicle")
                .await
                .unwrap();
            assert_eq!(ai_task.task_id, "ai-op-ai-rpc");
            assert_eq!(
                control.cancel_ai(&ai_task.task_id).await.unwrap().state,
                guard::api::v2::model::AiTaskSummaryState::Cancelled
            );
            assert_eq!(
                control
                    .stop_stream(&stream.stream_id)
                    .await
                    .unwrap()
                    .stream_id,
                stream.stream_id
            );

            let operations = OperationService::default();
            let executor = MqttCommandExecutor::new(operations.clone(), store);
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-live-1".to_string(),
                    action: CommandAction::StreamStart,
                    target: "device-2".to_string(),
                    payload: base::serde_json::json!({ "channel_id": "channel-2" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-live-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-playback-1".to_string(),
                    action: CommandAction::StreamPlayback,
                    target: "device-2".to_string(),
                    payload: base::serde_json::json!({ "channel_id": "channel-2" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-playback-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-download-1".to_string(),
                    action: CommandAction::StreamDownload,
                    target: "device-2".to_string(),
                    payload: base::serde_json::json!({ "channel_id": "channel-2" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-download-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-talk-1".to_string(),
                    action: CommandAction::StreamTalk,
                    target: "device-2".to_string(),
                    payload: base::serde_json::json!({ "channel_id": "channel-2" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-talk-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-ptz-1".to_string(),
                    action: CommandAction::Ptz,
                    target: "device-2".to_string(),
                    payload: base::serde_json::json!({ "channel_id": "channel-2" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-ptz-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-ai-1".to_string(),
                    action: CommandAction::AiStart,
                    target: "live-mqtt-live-1".to_string(),
                    payload: base::serde_json::json!({ "model": "vehicle" }),
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-ai-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-ai-cancel-1".to_string(),
                    action: CommandAction::AiCancel,
                    target: "ai-mqtt-ai-1".to_string(),
                    payload: base::serde_json::Value::Null,
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-ai-cancel-1").unwrap().status,
                OperationStatus::Succeeded
            );
            executor
                .execute(RoutedCommand {
                    command_id: "mqtt-stop-1".to_string(),
                    action: CommandAction::StreamStop,
                    target: "live-mqtt-live-1".to_string(),
                    payload: base::serde_json::Value::Null,
                })
                .await
                .unwrap();
            assert_eq!(
                operations.get("mqtt-stop-1").unwrap().status,
                OperationStatus::Succeeded
            );
        });
}

fn free_loopback_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn grpc_endpoint(addr: SocketAddr) -> EndpointRecord {
    EndpointRecord {
        name: "grpc".to_string(),
        scheme: "grpc".to_string(),
        host: addr.ip().to_string(),
        port: u32::from(addr.port()),
        mode: EndpointModeRecord::Single,
        labels: HashMap::new(),
    }
}

#[derive(Debug, Clone)]
struct FakeSession;

#[tonic::async_trait]
impl SessionControl for FakeSession {
    async fn start_live(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(fake_device_response(
            request.into_inner(),
            "live",
        )))
    }

    async fn start_playback(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(fake_device_response(
            request.into_inner(),
            "playback",
        )))
    }

    async fn start_download(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(fake_device_response(
            request.into_inner(),
            "download",
        )))
    }

    async fn start_talk(
        &self,
        request: tonic::Request<StartDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(fake_device_response(
            request.into_inner(),
            "talk",
        )))
    }

    async fn stop_device_stream(
        &self,
        request: tonic::Request<StopDeviceStreamRequest>,
    ) -> Result<tonic::Response<DeviceStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(DeviceStreamResponse {
            stream_id: request.into_inner().stream_id,
            state: DeviceStreamState::Stopped as i32,
            error: None,
            endpoint: String::new(),
        }))
    }

    async fn control_ptz(
        &self,
        _request: tonic::Request<ControlPtzRequest>,
    ) -> Result<tonic::Response<ControlPtzResponse>, tonic::Status> {
        Ok(tonic::Response::new(ControlPtzResponse {
            accepted: true,
            error: None,
        }))
    }

    async fn get_session_config(
        &self,
        _request: tonic::Request<GetSessionConfigRequest>,
    ) -> Result<tonic::Response<GetSessionConfigResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetSessionConfigResponse {
            domain: "3402000000".to_string(),
            domain_id: "34020000002000000001".to_string(),
            wan_ip: "101.33.200.169".to_string(),
            wan_port: 25600,
        }))
    }
    async fn list_gb_devices(
        &self,
        _request: tonic::Request<ListGbDevicesRequest>,
    ) -> Result<tonic::Response<ListGbDevicesResponse>, tonic::Status> {
        Ok(tonic::Response::new(ListGbDevicesResponse {
            devices: vec![],
        }))
    }

    async fn get_gb_device(
        &self,
        request: tonic::Request<GetGbDeviceRequest>,
    ) -> Result<tonic::Response<GetGbDeviceResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetGbDeviceResponse {
            device: Some(GbDevice {
                device_id: request.into_inner().device_id,
                session_node_id: "session-gb-online".to_string(),
                status: 1,
                ..Default::default()
            }),
        }))
    }

    async fn create_gb_device(
        &self,
        request: tonic::Request<CreateGbDeviceRequest>,
    ) -> Result<tonic::Response<CreateGbDeviceResponse>, tonic::Status> {
        let mut device = request.into_inner().device.unwrap_or_default();
        if device.device_id.is_empty() {
            device.device_id = "34020000001320000001".to_string();
        }
        device.session_node_id = "session-gb-online".to_string();
        Ok(tonic::Response::new(CreateGbDeviceResponse {
            device: Some(device),
        }))
    }

    async fn list_gb_channels(
        &self,
        _request: tonic::Request<ListGbChannelsRequest>,
    ) -> Result<tonic::Response<ListGbChannelsResponse>, tonic::Status> {
        Ok(tonic::Response::new(ListGbChannelsResponse {
            channels: vec![],
        }))
    }

    async fn get_gb_channel(
        &self,
        _request: tonic::Request<GetGbChannelRequest>,
    ) -> Result<tonic::Response<GetGbChannelResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetGbChannelResponse { channel: None }))
    }

    async fn list_gb_channel_images(
        &self,
        _request: tonic::Request<ListGbChannelImagesRequest>,
    ) -> Result<tonic::Response<ListGbChannelImagesResponse>, tonic::Status> {
        Ok(tonic::Response::new(ListGbChannelImagesResponse {
            images: vec![],
        }))
    }

    async fn snapshot_image(
        &self,
        _request: tonic::Request<SnapshotImageRequest>,
    ) -> Result<tonic::Response<SnapshotImageResponse>, tonic::Status> {
        Ok(tonic::Response::new(SnapshotImageResponse {
            session_id: "snapshot-session".to_string(),
            error: None,
        }))
    }
}

#[derive(Debug, Clone)]
struct FakeStream;

#[tonic::async_trait]
impl StreamControl for FakeStream {
    async fn start_receive(
        &self,
        request: tonic::Request<StartReceiveRequest>,
    ) -> Result<tonic::Response<StartReceiveResponse>, tonic::Status> {
        let request = request.into_inner();
        Ok(tonic::Response::new(StartReceiveResponse {
            stream_id: request.stream_id,
            state: StreamState::Receiving as i32,
            receive_endpoints: request.preferred_endpoints,
            error: None,
        }))
    }

    async fn stop_receive(
        &self,
        _request: tonic::Request<StopReceiveRequest>,
    ) -> Result<tonic::Response<StopReceiveResponse>, tonic::Status> {
        Ok(tonic::Response::new(StopReceiveResponse {
            state: StreamState::Stopped as i32,
            error: None,
        }))
    }

    async fn query_stream(
        &self,
        request: tonic::Request<QueryStreamRequest>,
    ) -> Result<tonic::Response<QueryStreamResponse>, tonic::Status> {
        Ok(tonic::Response::new(QueryStreamResponse {
            stream_id: request.into_inner().stream_id,
            state: StreamState::Receiving as i32,
            outputs: vec![],
        }))
    }

    async fn create_output(
        &self,
        _request: tonic::Request<CreateOutputRequest>,
    ) -> Result<tonic::Response<CreateOutputResponse>, tonic::Status> {
        Ok(tonic::Response::new(CreateOutputResponse {
            output_id: "out".to_string(),
            endpoints: vec![],
            error: None,
        }))
    }

    async fn close_output(
        &self,
        _request: tonic::Request<CloseOutputRequest>,
    ) -> Result<tonic::Response<CloseOutputResponse>, tonic::Status> {
        Ok(tonic::Response::new(CloseOutputResponse {
            closed: true,
            error: None,
        }))
    }

    async fn get_playback_endpoints(
        &self,
        _request: tonic::Request<GetPlaybackEndpointsRequest>,
    ) -> Result<tonic::Response<GetPlaybackEndpointsResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetPlaybackEndpointsResponse {
            endpoints: vec![],
        }))
    }

    async fn init_media(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamUnitResponse { error: None }))
    }

    async fn init_media_ext(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamUnitResponse { error: None }))
    }

    async fn stream_online(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamBoolResponse {
            value: true,
            error: None,
        }))
    }

    async fn record_info(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamJsonResponse {
            payload_json: vec![],
            error: None,
        }))
    }

    async fn close_output_by_ssrc(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamUnitResponse { error: None }))
    }

    async fn talk_open(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamJsonResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamJsonResponse {
            payload_json: vec![],
            error: None,
        }))
    }

    async fn talk_answer(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamUnitResponse { error: None }))
    }

    async fn talk_close(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamUnitResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamUnitResponse { error: None }))
    }

    async fn talk_online(
        &self,
        _request: tonic::Request<StreamJsonRequest>,
    ) -> Result<tonic::Response<StreamBoolResponse>, tonic::Status> {
        Ok(tonic::Response::new(StreamBoolResponse {
            value: true,
            error: None,
        }))
    }
}

fn fake_device_response(request: StartDeviceStreamRequest, prefix: &str) -> DeviceStreamResponse {
    let stream_id = request
        .operation
        .and_then(|operation| {
            (!operation.idempotency_key.is_empty()).then_some(operation.idempotency_key)
        })
        .map(|id| format!("{prefix}-{id}"))
        .unwrap_or_default();
    let endpoint = format!("rtp://127.0.0.1:30000/{stream_id}");
    DeviceStreamResponse {
        stream_id,
        state: DeviceStreamState::Running as i32,
        error: None,
        endpoint,
    }
}

#[derive(Debug, Clone)]
struct FakeAvai;

#[tonic::async_trait]
impl AvaiControl for FakeAvai {
    async fn create_task(
        &self,
        request: tonic::Request<CreateTaskRequest>,
    ) -> Result<tonic::Response<CreateTaskResponse>, tonic::Status> {
        Ok(tonic::Response::new(CreateTaskResponse {
            task_id: request.into_inner().task_id,
            state: AiTaskState::Running as i32,
            error: None,
        }))
    }

    async fn cancel_task(
        &self,
        _request: tonic::Request<CancelTaskRequest>,
    ) -> Result<tonic::Response<CancelTaskResponse>, tonic::Status> {
        Ok(tonic::Response::new(CancelTaskResponse {
            state: AiTaskState::Cancelled as i32,
            error: None,
        }))
    }

    async fn query_task(
        &self,
        request: tonic::Request<QueryTaskRequest>,
    ) -> Result<tonic::Response<QueryTaskResponse>, tonic::Status> {
        Ok(tonic::Response::new(QueryTaskResponse {
            task_id: request.into_inner().task_id,
            state: AiTaskState::Running as i32,
            result: vec![],
            error: None,
        }))
    }

    async fn query_capabilities(
        &self,
        _request: tonic::Request<QueryCapabilitiesRequest>,
    ) -> Result<tonic::Response<QueryCapabilitiesResponse>, tonic::Status> {
        Ok(tonic::Response::new(QueryCapabilitiesResponse {
            capabilities: vec!["ai.vehicle".to_string()],
            page: Some(PageResponse {
                next_page_token: String::new(),
            }),
        }))
    }
}
