use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use gmv_nodec::error::META_GLOBAL_CODE;
use gmv_protocol::avai::v1::avai_control_client::AvaiControlClient;
use gmv_protocol::avai::v1::{AiTaskState, CancelTaskRequest, CreateTaskRequest};
use gmv_protocol::common::v1::{
    ErrorDetail, NodeIdentity as ProtoIdentity, NodeKind as ProtoNodeKind, OperationRef,
};
use gmv_protocol::session::v1::session_control_client::SessionControlClient;
use gmv_protocol::session::v1::{
    ControlPtzRequest, CreateGbDeviceRequest, DeleteGbDeviceRequest, DeviceStreamState, GbChannel,
    GbChannelImage, GbDevice, GetGbChannelRequest, GetGbDeviceRequest, GetSessionConfigRequest,
    ListGbChannelImagesRequest, ListGbChannelsRequest, ListGbDevicesRequest, SnapshotImageRequest,
    StartDeviceStreamRequest, StopDeviceStreamRequest, UpdateGbChannelRequest,
    UpdateGbDeviceRequest,
};

use crate::api::v2::model::{AiTaskSummary, AiTaskSummaryState, StreamSummary, StreamSummaryState};
use crate::core::{
    ConnectionState, GmvGuardErrorCode, GuardError, GuardResult, LeaseState, NodeIdentity,
    NodeKind, RouteState, SchedulingState,
};
use crate::gateway::{AllocationRequest, AllocationService};
use crate::lease::{LeaseRequest, LeaseService};
use crate::route::{ResourceSnapshot, RouteService, SnapshotResource};
use crate::store::InMemoryGuardStore;
use crate::store::model::{NodeRecord, RouteRecord};

#[derive(Debug, Clone)]
pub struct BusinessControl {
    store: InMemoryGuardStore,
}

#[derive(Debug, Clone, Default)]
pub struct GbSessionConfigSummary {
    pub domain: String,
    pub domain_id: String,
    pub wan_ip: String,
    pub wan_port: u32,
}

#[derive(Debug, Clone, Default)]
pub struct GbDevicePage {
    pub devices: Vec<GbDevice>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceStreamOptions {
    pub token: String,
    pub start_time_sec: u32,
    pub end_time_sec: u32,
    pub trans_mode: String,
    pub output_type: String,
    pub talk_codec: String,
    pub talk_sample_rate: u32,
    pub talk_channel_count: u32,
    pub talk_frame_duration_ms: u32,
}

struct RpcEdge<'a> {
    service: &'a str,
    action: &'a str,
    node_id: &'a str,
    operation_id: &'a str,
    resource_id: &'a str,
    started: Instant,
}

impl<'a> RpcEdge<'a> {
    fn new(
        service: &'a str,
        action: &'a str,
        node_id: &'a str,
        operation_id: &'a str,
        resource_id: &'a str,
    ) -> Self {
        Self {
            service,
            action,
            node_id,
            operation_id,
            resource_id,
            started: Instant::now(),
        }
    }

    fn success(&self) {
        base::log::debug!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=success, elapsed_ms={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis()
        );
    }

    fn transport_error(&self, error: tonic::Status) -> GuardError {
        let global_code = error
            .metadata()
            .get(META_GLOBAL_CODE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=transport_error, elapsed_ms={}, tonic_code={:?}, global_code={}, reason={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            error.code(),
            global_code,
            error.message()
        );
        node_rpc_status(self.service, self.action, error)
    }

    fn response<T>(&self, response: Result<tonic::Response<T>, tonic::Status>) -> GuardResult<T> {
        response
            .map(tonic::Response::into_inner)
            .map_err(|error| self.transport_error(error))
    }

    fn business_rejection(&self, error: &ErrorDetail) {
        let global_code = error
            .metadata
            .get(META_GLOBAL_CODE)
            .map(String::as_str)
            .unwrap_or("");
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=business_rejection, elapsed_ms={}, remote_code={}, global_code={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            error.code,
            global_code
        );
    }

    fn invalid_response(&self, reason: &str) {
        base::log::warn!(
            "guard rpc edge result: service={}, action={}, node_id={}, operation_id={}, resource_id={}, outcome=invalid_response, elapsed_ms={}, reason={}",
            self.service,
            self.action,
            self.node_id,
            self.operation_id,
            self.resource_id,
            self.started.elapsed().as_millis(),
            reason
        );
    }
}

impl BusinessControl {
    pub fn new(store: InMemoryGuardStore) -> Self {
        Self { store }
    }

    pub async fn gb_session_config(&self, node_id: &str) -> GuardResult<GbSessionConfigSummary> {
        let session = self
            .store
            .get_node(node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "get_session_config", node_id));
        }
        let mut client = self.session_client(&session).await?;
        let request = GetSessionConfigRequest {};
        base::log::debug!(
            "guard rpc client outbound: method=session_control.get_session_config, node={}, req:<empty>",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "get_session_config",
            &session.identity.node_id,
            "",
            "",
        );
        let response = edge.response(client.get_session_config(request).await)?;
        edge.success();
        Ok(GbSessionConfigSummary {
            domain: response.domain,
            domain_id: response.domain_id,
            wan_ip: response.wan_ip,
            wan_port: response.wan_port,
        })
    }

    pub async fn first_gb_session_node_by_domain(&self) -> GuardResult<(String, String)> {
        let mut options = Vec::new();
        for session in self.session_nodes() {
            if let Ok(config) = self.gb_session_config(&session.identity.node_id).await
                && !config.domain_id.is_empty()
            {
                options.push((session.identity.node_id, config.domain_id));
            }
        }
        options.sort_by(|left, right| left.1.cmp(&right.1));
        options
            .into_iter()
            .next()
            .ok_or_else(|| GuardError::NotFound("GB28181 session node with domain_id".to_string()))
    }

    pub async fn list_gb_devices(&self) -> GuardResult<Vec<GbDevice>> {
        let mut devices = Vec::new();
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = ListGbDevicesRequest {
                page: 0,
                page_size: 0,
                domain_id: String::new(),
                device_id: String::new(),
                device_name: String::new(),
                registered_only: false,
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.list_gb_devices, node={}, req:{request:?}",
                session.identity.node_id,
            );
            let edge = RpcEdge::new(
                "session",
                "list_gb_devices",
                &session.identity.node_id,
                "",
                "",
            );
            let mut response = edge.response(client.list_gb_devices(request).await)?;
            edge.success();
            for device in &mut response.devices {
                if device.session_node_id.is_empty() {
                    device.session_node_id = session.identity.node_id.clone();
                }
            }
            devices.extend(response.devices);
        }
        devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        Ok(devices)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_gb_device_page(
        &self,
        session_node_id: &str,
        domain_id: &str,
        device_id: &str,
        device_name: &str,
        registered_only: bool,
        page: u32,
        page_size: u32,
    ) -> GuardResult<GbDevicePage> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        let page = page.max(1);
        let page_size = page_size.max(1);
        let mut client = self.session_client(&session).await?;
        let request = ListGbDevicesRequest {
            page,
            page_size,
            domain_id: domain_id.to_string(),
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            registered_only,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.list_gb_devices, node={}, req:{request:?}",
            session.identity.node_id,
        );
        let edge = RpcEdge::new(
            "session",
            "list_gb_devices",
            &session.identity.node_id,
            "",
            device_id,
        );
        let mut response = edge.response(client.list_gb_devices(request).await)?;
        edge.success();
        for device in &mut response.devices {
            if device.session_node_id.is_empty() {
                device.session_node_id = session.identity.node_id.clone();
            }
        }
        Ok(GbDevicePage {
            devices: response.devices,
            total: response.total,
            page: response.page,
            page_size: response.page_size,
        })
    }

    pub async fn create_gb_device(&self, mut device: GbDevice) -> GuardResult<GbDevice> {
        let node_id = device.session_node_id.clone();
        let resource_id = device.device_id.clone();
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "create_gb_device", &node_id));
        }
        device.session_node_id.clear();
        base::log::debug!(
            "guard rpc client outbound: method=session_control.create_gb_device, node={}, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
            session.identity.node_id,
            device.device_id,
            device.session_node_id,
            device.domain_id,
            device.domain,
            device.longitude,
            device.latitude,
            device.address,
            if device.pwd.is_empty() {
                "<empty>"
            } else {
                "<redacted>"
            },
            device.pwd_check,
            device.alias,
            device.status,
            device.heartbeat_sec,
            device.tenant_id,
            device.sys_org_code,
            device.create_by,
            device.update_by
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "create_gb_device",
            &session.identity.node_id,
            "",
            &resource_id,
        );
        let response = edge.response(
            client
                .create_gb_device(CreateGbDeviceRequest {
                    device: Some(device),
                })
                .await,
        )?;
        let Some(mut response) = response.device else {
            edge.invalid_response("empty_device");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 device".to_string(),
            ));
        };
        edge.success();
        if response.session_node_id.is_empty() {
            response.session_node_id = session.identity.node_id;
        }
        Ok(response)
    }

    pub async fn update_gb_device(&self, mut device: GbDevice) -> GuardResult<GbDevice> {
        let node_id = device.session_node_id.clone();
        let resource_id = device.device_id.clone();
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "update_gb_device", &node_id));
        }
        device.session_node_id.clear();
        base::log::debug!(
            "guard rpc client outbound: method=session_control.update_gb_device, node={}, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
            session.identity.node_id,
            device.device_id,
            device.session_node_id,
            device.domain_id,
            device.domain,
            device.longitude,
            device.latitude,
            device.address,
            if device.pwd.is_empty() {
                "<empty>"
            } else {
                "<redacted>"
            },
            device.pwd_check,
            device.alias,
            device.status,
            device.heartbeat_sec,
            device.tenant_id,
            device.sys_org_code,
            device.create_by,
            device.update_by
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "update_gb_device",
            &session.identity.node_id,
            "",
            &resource_id,
        );
        let response = edge.response(
            client
                .update_gb_device(UpdateGbDeviceRequest {
                    device: Some(device),
                })
                .await,
        )?;
        let Some(mut response) = response.device else {
            edge.invalid_response("empty_device");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 device".to_string(),
            ));
        };
        edge.success();
        if response.session_node_id.is_empty() {
            response.session_node_id = session.identity.node_id;
        }
        Ok(response)
    }

    pub async fn delete_gb_device(
        &self,
        session_node_id: &str,
        device_id: &str,
        domain_id: &str,
    ) -> GuardResult<()> {
        let session = self.store.get_node(session_node_id).ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 session node {session_node_id}"))
        })?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {session_node_id}"
            )));
        }
        let request = DeleteGbDeviceRequest {
            device_id: device_id.to_string(),
            domain_id: domain_id.to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.delete_gb_device, node={}, req:{request:?}",
            session.identity.node_id,
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "delete_gb_device",
            &session.identity.node_id,
            "",
            device_id,
        );
        edge.response(client.delete_gb_device(request).await)?;
        edge.success();
        Ok(())
    }

    pub async fn get_gb_device(&self, device_id: &str) -> GuardResult<Option<GbDevice>> {
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = GetGbDeviceRequest {
                device_id: device_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.get_gb_device, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "get_gb_device",
                &session.identity.node_id,
                "",
                device_id,
            );
            let response = edge.response(client.get_gb_device(request).await)?;
            edge.success();
            if let Some(mut device) = response.device {
                if device.session_node_id.is_empty() {
                    device.session_node_id = session.identity.node_id;
                }
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    pub async fn list_gb_channels(&self, device_id: &str) -> GuardResult<Vec<GbChannel>> {
        let mut channels = Vec::new();
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = ListGbChannelsRequest {
                device_id: device_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.list_gb_channels, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "list_gb_channels",
                &session.identity.node_id,
                "",
                device_id,
            );
            let response = edge.response(client.list_gb_channels(request).await)?;
            edge.success();
            channels.extend(response.channels);
        }
        channels.sort_by(|left, right| {
            left.sort_no
                .cmp(&right.sort_no)
                .then_with(|| left.channel_id.cmp(&right.channel_id))
        });
        Ok(channels)
    }

    pub async fn get_gb_channel(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Option<GbChannel>> {
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = GetGbChannelRequest {
                device_id: device_id.to_string(),
                channel_id: channel_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.get_gb_channel, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "get_gb_channel",
                &session.identity.node_id,
                "",
                channel_id,
            );
            let response = edge.response(client.get_gb_channel(request).await)?;
            edge.success();
            if response.channel.is_some() {
                return Ok(response.channel);
            }
        }
        Ok(None)
    }

    pub async fn update_gb_channel(&self, channel: GbChannel) -> GuardResult<GbChannel> {
        let device_id = channel.device_id.clone();
        let channel_id = channel.channel_id.clone();
        let device = self
            .get_gb_device(&device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
        let node_id = device.session_node_id;
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "update_gb_channel", &node_id));
        }
        let request = UpdateGbChannelRequest {
            channel: Some(channel),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.update_gb_channel, node={}, req: device_id={}, channel_id={}",
            session.identity.node_id,
            device_id,
            channel_id,
        );
        let mut client = self.session_client(&session).await?;
        let edge = RpcEdge::new(
            "session",
            "update_gb_channel",
            &session.identity.node_id,
            "",
            &channel_id,
        );
        let response = edge.response(client.update_gb_channel(request).await)?;
        let Some(channel) = response.channel else {
            edge.invalid_response("empty_channel");
            return Err(GuardError::Conflict(
                "session returned empty GB28181 channel".to_string(),
            ));
        };
        edge.success();
        Ok(channel)
    }

    pub async fn list_gb_channel_images(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Vec<GbChannelImage>> {
        let mut images = Vec::new();
        for session in self.session_nodes() {
            let mut client = self.session_client(&session).await?;
            let request = ListGbChannelImagesRequest {
                device_id: device_id.to_string(),
                channel_id: channel_id.to_string(),
            };
            base::log::debug!(
                "guard rpc client outbound: method=session_control.list_gb_channel_images, node={}, req:{request:?}",
                session.identity.node_id
            );
            let edge = RpcEdge::new(
                "session",
                "list_gb_channel_images",
                &session.identity.node_id,
                "",
                channel_id,
            );
            let response = edge.response(client.list_gb_channel_images(request).await)?;
            edge.success();
            images.extend(response.images);
        }
        images.sort_by_key(|image| std::cmp::Reverse(image.created_at_ms));
        Ok(images)
    }

    pub async fn snapshot_image(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        count: u32,
        interval: u32,
    ) -> GuardResult<String> {
        let device = self
            .get_gb_device(device_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
        let node_id = device.session_node_id;
        let session = self
            .store
            .get_node(&node_id)
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 session node {node_id}")))?;
        if !is_gb_session_node(&session) {
            return Err(GuardError::NotFound(format!(
                "GB28181 session node {node_id}"
            )));
        }
        if session.connection != ConnectionState::Connected
            || session.scheduling != SchedulingState::Enabled
        {
            return Err(node_unavailable("session", "snapshot_image", &node_id));
        }
        let mut client = self.session_client(&session).await?;
        let request = SnapshotImageRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            count,
            interval,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.snapshot_image, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "snapshot_image",
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let response = edge.response(client.snapshot_image(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "snapshot_image",
                error,
                "snapshot_rejected",
                "抓拍请求未被设备接受，请确认设备在线且支持抓拍",
                true,
            ));
        }
        if response.session_id.is_empty() {
            edge.invalid_response("empty_session_id");
            return Err(GuardError::Conflict(
                "session snapshot returned empty session id".to_string(),
            ));
        }
        edge.success();
        Ok(response.session_id)
    }

    pub async fn start_live(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_live_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_live_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Live,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_playback(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_playback_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_playback_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Playback,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_download(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_download_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_download_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Download,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    pub async fn start_talk(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<StreamSummary> {
        self.start_talk_with_options(
            operation_id,
            device_id,
            channel_id,
            DeviceStreamOptions::default(),
        )
        .await
    }

    pub async fn start_talk_with_options(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        self.start_device_stream(
            DeviceStreamKind::Talk,
            operation_id,
            device_id,
            channel_id,
            options,
        )
        .await
    }

    async fn start_device_stream(
        &self,
        kind: DeviceStreamKind,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        options: DeviceStreamOptions,
    ) -> GuardResult<StreamSummary> {
        let session = self.select_node(NodeKind::Session, kind.session_capability())?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let operation = OperationRef {
            operation_id: operation_id.to_string(),
            idempotency_key: operation_id.to_string(),
        };
        let token = if options.token.trim().is_empty() {
            format!("gmv-{operation_id}")
        } else {
            options.token
        };
        let request = StartDeviceStreamRequest {
            operation: Some(operation),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            route_id: String::new(),
            lease_id: String::new(),
            expected_session: Some(proto_identity(&session.identity)),
            token,
            start_time_sec: options.start_time_sec,
            end_time_sec: options.end_time_sec,
            trans_mode: options.trans_mode,
            output_type: options.output_type,
            talk_codec: options.talk_codec,
            talk_sample_rate: options.talk_sample_rate,
            talk_channel_count: options.talk_channel_count,
            talk_frame_duration_ms: options.talk_frame_duration_ms,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.start_{}, node={}, req: operation={:?}, device_id={}, channel_id={}, token={}, start_time_sec={}, end_time_sec={}, trans_mode={}, output_type={}, talk_codec={}, talk_sample_rate={}, talk_channel_count={}, talk_frame_duration_ms={}, expected_session={:?}",
            kind.prefix(),
            session.identity.node_id,
            request.operation,
            request.device_id,
            request.channel_id,
            if request.token.is_empty() {
                "<empty>"
            } else {
                "<redacted>"
            },
            request.start_time_sec,
            request.end_time_sec,
            request.trans_mode,
            request.output_type,
            request.talk_codec,
            request.talk_sample_rate,
            request.talk_channel_count,
            request.talk_frame_duration_ms,
            request.expected_session
        );
        let edge = RpcEdge::new(
            "session",
            kind.action(),
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let session_response = edge.response(match kind {
            DeviceStreamKind::Live => session_client.start_live(request).await,
            DeviceStreamKind::Playback => session_client.start_playback(request).await,
            DeviceStreamKind::Download => session_client.start_download(request).await,
            DeviceStreamKind::Talk => session_client.start_talk(request).await,
        })?;
        if let Some(error) = non_empty_error(session_response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                kind.action(),
                error,
                "stream_start_failed",
                "视频流创建失败，请检查设备在线状态和媒体服务",
                true,
            ));
        }
        if session_response.state != DeviceStreamState::Running as i32 {
            edge.invalid_response("stream_not_running");
            return Err(GuardError::Conflict(format!(
                "session did not enter {} running state",
                kind.prefix()
            )));
        }
        edge.success();
        let lease = self
            .store
            .leases()
            .into_iter()
            .find(|lease| lease.resource_id == session_response.stream_id);
        let route = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == session_response.stream_id);
        Ok(StreamSummary {
            stream_id: session_response.stream_id,
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            node_id: route
                .as_ref()
                .map(|route| route.node_id.clone())
                .unwrap_or_else(|| session.identity.node_id.clone()),
            instance_id: route
                .as_ref()
                .map(|route| route.instance_id.clone())
                .unwrap_or_else(|| session.identity.instance_id.clone()),
            lease_id: lease.map(|lease| lease.lease_id).unwrap_or_default(),
            route_id: route.map(|route| route.route_id).unwrap_or_default(),
            endpoint: session_response.endpoint,
            video_codec: session_response.video_codec,
            audio_codec: session_response.audio_codec,
            state: StreamSummaryState::Running,
        })
    }

    pub async fn stop_stream(
        &self,
        operation_id: &str,
        stream_id: &str,
    ) -> GuardResult<StreamSummary> {
        let session = self.select_any_session()?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let request = StopDeviceStreamRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            stream_id: stream_id.to_string(),
            reason: "manual".to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.stop_device_stream, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "stop_device_stream",
            &session.identity.node_id,
            operation_id,
            stream_id,
        );
        let response = edge.response(session_client.stop_device_stream(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "session",
                "stop_device_stream",
                error,
                "stream_stop_failed",
                "停止视频流失败，请稍后重试",
                true,
            ));
        }
        edge.success();
        if let Some(route) = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == stream_id && route.state != RouteState::Closed)
            && let Some(mut stored_route) = self.store.get_route(&route.route_id)
        {
            stored_route.state = RouteState::Closed;
            self.store.upsert_route(stored_route);
        }
        Ok(StreamSummary {
            stream_id: stream_id.to_string(),
            device_id: String::new(),
            channel_id: String::new(),
            node_id: session.identity.node_id,
            instance_id: session.identity.instance_id,
            lease_id: String::new(),
            route_id: String::new(),
            endpoint: String::new(),
            video_codec: String::new(),
            audio_codec: String::new(),
            state: StreamSummaryState::Stopped,
        })
    }

    pub async fn ptz(
        &self,
        operation_id: &str,
        device_id: &str,
        channel_id: &str,
        command: &str,
        speed: u32,
    ) -> GuardResult<u64> {
        let session = self.select_node(NodeKind::Session, "device.ptz")?;
        let session_grpc = grpc_uri(&session)?;
        let mut session_client =
            SessionControlClient::new(connect_rpc(&session_grpc, "session").await?);
        let request = ControlPtzRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            command: command.to_string(),
            speed,
        };
        base::log::debug!(
            "guard rpc client outbound: method=session_control.control_ptz, node={}, req:{request:?}",
            session.identity.node_id
        );
        let edge = RpcEdge::new(
            "session",
            "control_ptz",
            &session.identity.node_id,
            operation_id,
            device_id,
        );
        let response = edge.response(session_client.control_ptz(request).await)?;
        if !response.accepted {
            if let Some(error) = response.error.as_ref() {
                edge.business_rejection(error);
            } else {
                edge.invalid_response("ptz_not_accepted_without_error");
            }
            return Err(response
                .error
                .filter(|error| !error.code.is_empty() || !error.message.is_empty())
                .map(|error| {
                    remote_error(
                        "session",
                        "control_ptz",
                        error,
                        "ptz_rejected",
                        "云台控制未被设备接受，请确认通道支持云台",
                        false,
                    )
                })
                .unwrap_or_else(|| {
                    user_error(
                        "ptz_rejected",
                        "session ptz rejected",
                        "云台控制未被设备接受，请确认通道支持云台",
                        false,
                        detail_pairs([("service", "session"), ("action", "control_ptz")]),
                    )
                }));
        }
        edge.success();
        Ok(1)
    }

    pub async fn start_ai(
        &self,
        operation_id: &str,
        stream_id: &str,
        model: &str,
    ) -> GuardResult<AiTaskSummary> {
        let capability = ai_capability(model);
        let avai = self.select_node(NodeKind::Avai, &capability)?;
        let task_id = format!("ai-{operation_id}");
        let lease_id = format!("lease-ai-{operation_id}");
        let route_id = format!("route-ai-{operation_id}");
        let allocation =
            AllocationService::new(self.store.clone()).allocate(AllocationRequest {
                request_id: operation_id.to_string(),
                resource_id: task_id.clone(),
                capability: capability.clone(),
                zone: avai.zone.clone(),
                constraints: std::collections::HashMap::new(),
            })?;
        if allocation.owner.node_id != avai.identity.node_id {
            return Err(GuardError::Conflict(
                "selected avai node changed during allocation".to_string(),
            ));
        }
        LeaseService::new(self.store.clone()).allocate(LeaseRequest {
            lease_id: lease_id.clone(),
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            stream_type: capability.clone(),
            idempotency_key: format!("ai-{operation_id}"),
            owner: avai.identity.clone(),
            constraints: std::collections::HashMap::new(),
            now_ms: now_ms(),
            ttl_ms: 30_000,
        })?;
        RouteService::new(self.store.clone()).create_allocated(RouteRecord {
            route_id: route_id.clone(),
            resource_id: task_id.clone(),
            node_id: avai.identity.node_id.clone(),
            instance_id: avai.identity.instance_id.clone(),
            state: RouteState::Allocated,
            desired_generation: 1,
            observed_generation: 0,
            observed_sequence: 0,
        })?;

        let avai_grpc = grpc_uri(&avai)?;
        let mut avai_client = AvaiControlClient::new(connect_rpc(&avai_grpc, "avai").await?);
        let request = CreateTaskRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            task_id: task_id.clone(),
            task_type: capability.clone(),
            route_id: route_id.clone(),
            expected_avai: Some(proto_identity(&avai.identity)),
            payload: format!(
                "frame_ref={operation_id};stream_id={stream_id};expires_at_epoch_ms={}",
                now_ms() + 30_000
            )
            .into_bytes(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=avai_control.create_task, node={}, req: operation={:?}, task_id={}, task_type={}, route_id={}, expected_avai={:?}, payload_bytes={}",
            avai.identity.node_id,
            request.operation,
            request.task_id,
            request.task_type,
            request.route_id,
            request.expected_avai,
            request.payload.len()
        );
        let edge = RpcEdge::new(
            "avai",
            "create_task",
            &avai.identity.node_id,
            operation_id,
            &task_id,
        );
        let response = edge.response(avai_client.create_task(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            let _ =
                LeaseService::new(self.store.clone()).fail(&lease_id, &avai.identity.instance_id);
            return Err(remote_error(
                "avai",
                "create_task",
                error,
                "avai_task_rejected",
                "AI 任务创建失败，请检查目标节点状态后重试",
                true,
            ));
        }
        if response.state != AiTaskState::Running as i32 {
            edge.invalid_response("task_not_running");
            return Err(GuardError::Conflict(
                "avai task did not enter running state".to_string(),
            ));
        }
        edge.success();
        LeaseService::new(self.store.clone()).confirm(&lease_id, &avai.identity.instance_id)?;
        RouteService::new(self.store.clone()).apply_snapshot(ResourceSnapshot {
            owner: avai.identity.clone(),
            generation: 1,
            sequence: 1,
            resources: vec![SnapshotResource {
                resource_id: task_id.clone(),
                route_id: Some(route_id.clone()),
            }],
        })?;
        Ok(AiTaskSummary {
            task_id: response.task_id,
            model: model.to_string(),
            stream_id: stream_id.to_string(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id,
            route_id,
            state: AiTaskSummaryState::Running,
        })
    }

    pub async fn cancel_ai(&self, operation_id: &str, task_id: &str) -> GuardResult<AiTaskSummary> {
        let route = self
            .store
            .routes()
            .into_iter()
            .find(|route| route.resource_id == task_id && route.state != RouteState::Closed)
            .ok_or_else(|| GuardError::NotFound(format!("AI task {task_id}")))?;
        let avai = self
            .store
            .get_node(&route.node_id)
            .ok_or_else(|| GuardError::NotFound(format!("node {}", route.node_id)))?;
        let avai_grpc = grpc_uri(&avai)?;
        let mut avai_client = AvaiControlClient::new(connect_rpc(&avai_grpc, "avai").await?);
        let request = CancelTaskRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: String::new(),
            }),
            task_id: task_id.to_string(),
            reason: "manual".to_string(),
        };
        base::log::debug!(
            "guard rpc client outbound: method=avai_control.cancel_task, node={}, req:{request:?}",
            avai.identity.node_id
        );
        let edge = RpcEdge::new(
            "avai",
            "cancel_task",
            &avai.identity.node_id,
            operation_id,
            task_id,
        );
        let response = edge.response(avai_client.cancel_task(request).await)?;
        if let Some(error) = non_empty_error(response.error) {
            edge.business_rejection(&error);
            return Err(remote_error(
                "avai",
                "cancel_task",
                error,
                "avai_cancel_rejected",
                "AI 任务取消失败，请检查目标节点状态后重试",
                true,
            ));
        }
        if response.state != AiTaskState::Cancelled as i32 {
            edge.invalid_response("task_not_cancelled");
            return Err(GuardError::Conflict(
                "avai task did not enter cancelled state".to_string(),
            ));
        }
        edge.success();
        if let Some(mut stored_route) = self.store.get_route(&route.route_id) {
            stored_route.state = RouteState::Closed;
            self.store.upsert_route(stored_route);
        }
        if let Some(lease) = self
            .store
            .leases()
            .into_iter()
            .find(|lease| lease.resource_id == task_id && lease.state == LeaseState::Confirmed)
        {
            let _ = LeaseService::new(self.store.clone())
                .release(&lease.lease_id, &avai.identity.instance_id);
        }
        Ok(AiTaskSummary {
            task_id: task_id.to_string(),
            model: String::new(),
            stream_id: String::new(),
            node_id: avai.identity.node_id,
            instance_id: avai.identity.instance_id,
            lease_id: String::new(),
            route_id: route.route_id,
            state: AiTaskSummaryState::Cancelled,
        })
    }

    fn select_any_session(&self) -> GuardResult<NodeRecord> {
        self.session_nodes()
            .into_iter()
            .next()
            .ok_or_else(|| GuardError::NotFound("no connected session node".to_string()))
    }

    fn session_nodes(&self) -> Vec<NodeRecord> {
        let mut nodes = self
            .store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == NodeKind::Session
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id));
        nodes
    }

    async fn session_client(
        &self,
        session: &NodeRecord,
    ) -> GuardResult<SessionControlClient<tonic::transport::Channel>> {
        let session_grpc = grpc_uri(session)?;
        Ok(SessionControlClient::new(
            connect_rpc(&session_grpc, "session").await?,
        ))
    }

    fn select_node(&self, kind: NodeKind, capability: &str) -> GuardResult<NodeRecord> {
        self.store
            .nodes()
            .into_iter()
            .filter(|node| {
                node.identity.kind == kind
                    && node.connection == ConnectionState::Connected
                    && node.scheduling == SchedulingState::Enabled
                    && node.capabilities.iter().any(|item| item == capability)
            })
            .min_by(|left, right| left.identity.node_id.cmp(&right.identity.node_id))
            .ok_or_else(|| GuardError::NotFound(format!("no {:?} node for {capability}", kind)))
    }
}

async fn connect_rpc(uri: &str, name: &str) -> GuardResult<tonic::transport::Channel> {
    let mut config = base_rpc::RpcChannelConfig::new(uri.to_string());
    if uri.starts_with("https://") {
        config.tls = Some(base_rpc::RpcClientTlsConfig {
            domain_name: url::Url::parse(uri)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string)),
            ca_certificate_pem: None,
            client_certificate_pem: None,
            client_private_key_pem: None,
            use_native_roots: true,
            handshake_timeout: Duration::from_secs(5),
        });
    }
    let started = Instant::now();
    base::log::debug!("guard rpc client outbound: service={name}, endpoint={uri}");
    let channel = base_rpc::connect_channel(&config).await.map_err(|error| {
        base::log::debug!(
            "guard rpc client inbound: service={name}, endpoint={uri}, status=error, elapsed_ms={}, err={error}",
            started.elapsed().as_millis()
        );
        node_connect_error(name, error.to_string())
    })?;
    base::log::debug!(
        "guard rpc client inbound: service={name}, endpoint={uri}, status=ok, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(channel)
}

fn is_gb_session_node(node: &NodeRecord) -> bool {
    node.identity.kind == NodeKind::Session
        && (node.config.get("service").map(String::as_str) == Some("session-gb28181")
            || node.config.get("protocol").map(String::as_str) == Some("gb28181")
            || node
                .capabilities
                .iter()
                .any(|item| item == "protocol.gb28181"))
}
fn grpc_uri(node: &NodeRecord) -> GuardResult<String> {
    let endpoint = node
        .endpoints
        .iter()
        .find(|endpoint| {
            endpoint.name == "grpc" || matches!(endpoint.scheme.as_str(), "grpc" | "grpcs")
        })
        .ok_or_else(|| {
            user_error(
                "node_endpoint_missing",
                format!("node {} grpc endpoint missing", node.identity.node_id),
                "节点未上报 RPC 地址，请检查节点配置",
                false,
                detail_pairs([
                    ("node_id", node.identity.node_id.as_str()),
                    ("service", node_kind_name(node.identity.kind)),
                ]),
            )
        })?;
    let scheme = if endpoint.scheme == "grpcs" {
        "https"
    } else {
        "http"
    };
    Ok(format!("{scheme}://{}:{}", endpoint.host, endpoint.port))
}

fn proto_identity(identity: &NodeIdentity) -> ProtoIdentity {
    ProtoIdentity {
        node_id: identity.node_id.clone(),
        instance_id: identity.instance_id.clone(),
        kind: match identity.kind {
            NodeKind::Session => ProtoNodeKind::Session,
            NodeKind::Stream => ProtoNodeKind::Stream,
            NodeKind::Avai => ProtoNodeKind::Avai,
        } as i32,
    }
}

fn non_empty_error(error: Option<ErrorDetail>) -> Option<ErrorDetail> {
    error.filter(|error| !error.code.is_empty() || !error.message.is_empty())
}

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Session => "session",
        NodeKind::Stream => "stream",
        NodeKind::Avai => "avai",
    }
}

fn node_unavailable(service: &str, action: &str, node_id: &str) -> GuardError {
    user_error(
        "node_unavailable",
        format!("{service} node {node_id} is offline or disabled"),
        "节点离线或不可调度，请等待恢复或切换节点",
        true,
        detail_pairs([
            ("node_id", node_id),
            ("service", service),
            ("action", action),
        ]),
    )
}

fn node_connect_error(service: &str, message: String) -> GuardError {
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("deadline")
        || lower.contains("timeout")
        || lower.contains("timed out")
    {
        "node_rpc_timeout"
    } else if lower.contains("tls") || lower.contains("certificate") || lower.contains("handshake")
    {
        "node_rpc_tls_failed"
    } else {
        "node_rpc_connect_failed"
    };
    let user_message = match code {
        "node_rpc_timeout" => "节点响应超时，请稍后重试或检查节点负载/网络",
        "node_rpc_tls_failed" => "节点安全连接失败，请检查证书、域名或主机时间",
        _ => "无法连接目标节点，请检查节点进程、地址和网络",
    };
    user_error(
        code,
        format!("connect {service} RPC failed: {message}"),
        user_message,
        true,
        detail_pairs([("service", service)]),
    )
}

fn node_rpc_status(service: &str, action: &str, error: tonic::Status) -> GuardError {
    if let Some((global_code, output)) =
        gmv_nodec::error::global_error_output_from_tonic_status(&error)
    {
        let code = GmvGuardErrorCode::from_code(global_code)
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string());
        let mut details = detail_pairs([("service", service), ("action", action)]);
        details.insert("global_code".to_string(), global_code.to_string());
        details.insert("global_code_name".to_string(), output.code_name.to_string());
        details.insert("retryable".to_string(), output.retryable.to_string());
        return user_error(
            code,
            format!("{service} RPC {action} failed: {error}"),
            output.user_message,
            output.retryable,
            details,
        );
    }
    let code = match error.code() {
        tonic::Code::DeadlineExceeded => "node_rpc_timeout",
        tonic::Code::Unavailable => "node_rpc_unavailable",
        tonic::Code::Unauthenticated => "unauthorized",
        tonic::Code::PermissionDenied => "forbidden",
        tonic::Code::InvalidArgument => "bad_request",
        _ => "node_rpc_unavailable",
    };
    let user_message = match code {
        "node_rpc_timeout" => "节点响应超时，请稍后重试或检查节点负载/网络",
        "node_rpc_unavailable" => "无法连接目标节点，请检查节点进程、地址和网络",
        "unauthorized" => "节点认证失败，请检查服务间认证配置",
        "forbidden" => "目标节点拒绝执行此操作，请检查节点权限配置",
        "bad_request" => "请求参数不完整或不符合目标节点要求",
        _ => "节点调用失败，请稍后重试",
    };
    user_error(
        code,
        format!("{service} RPC {action} failed: {error}"),
        user_message,
        matches!(
            error.code(),
            tonic::Code::DeadlineExceeded | tonic::Code::Unavailable
        ),
        detail_pairs([("service", service), ("action", action)]),
    )
}

fn remote_error(
    service: &str,
    action: &str,
    error: ErrorDetail,
    fallback_code: &str,
    fallback_user_message: &str,
    retryable: bool,
) -> GuardError {
    let remote_code = error.code;
    let remote_message = error.message;
    let metadata = error.metadata;
    let registered_output = metadata
        .get(META_GLOBAL_CODE)
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|code| base::err::error_output(code).map(|output| (code, output)));
    let code = if let Some((global_code, output)) = &registered_output {
        GmvGuardErrorCode::from_code(*global_code)
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string())
    } else if remote_code.trim().is_empty() {
        fallback_code.to_string()
    } else {
        remote_code.clone()
    };
    let user_message = if let Some((_, output)) = &registered_output {
        output.user_message.to_string()
    } else if remote_message.trim().is_empty() {
        fallback_user_message.to_string()
    } else {
        remote_user_message(&code, &remote_message, fallback_user_message)
    };
    let retryable = registered_output
        .as_ref()
        .map_or(retryable, |(_, output)| output.retryable);
    let mut details = detail_pairs([("service", service), ("action", action)]);
    if !remote_code.trim().is_empty() {
        details.insert("remote_code".to_string(), remote_code.clone());
    }
    details.extend(metadata);
    let message = if remote_message.trim().is_empty() {
        format!("{service} RPC {action} rejected: {code}")
    } else {
        format!("{service} RPC {action} rejected: {remote_message}")
    };
    user_error(code, message, user_message, retryable, details)
}

fn remote_user_message(code: &str, message: &str, fallback: &str) -> String {
    if let Some(error_code) = GmvGuardErrorCode::from_api_code(code) {
        return error_code.out_msg().to_string();
    }
    if message.trim().is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback}：{message}")
    }
}

fn user_error(
    code: impl Into<String>,
    message: impl Into<String>,
    user_message: impl Into<String>,
    retryable: bool,
    details: BTreeMap<String, String>,
) -> GuardError {
    GuardError::user_visible(code, message, user_message, retryable, details)
}

fn detail_pairs<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[derive(Debug, Clone, Copy)]
enum DeviceStreamKind {
    Live,
    Playback,
    Download,
    Talk,
}

impl DeviceStreamKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Playback => "playback",
            Self::Download => "download",
            Self::Talk => "talk",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Live => "start_live",
            Self::Playback => "start_playback",
            Self::Download => "start_download",
            Self::Talk => "start_talk",
        }
    }

    fn session_capability(self) -> &'static str {
        match self {
            Self::Live => "device.live",
            Self::Playback => "device.playback",
            Self::Download => "device.download",
            Self::Talk => "device.talk",
        }
    }
}

fn ai_capability(model: &str) -> String {
    if model.starts_with("ai.") {
        model.to_string()
    } else {
        format!("ai.{model}")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gmv_protocol::common::v1::ErrorDetail;

    use super::*;

    #[test]
    fn remote_error_uses_global_code_metadata_for_user_message() {
        let error = ErrorDetail {
            code: "session_business_failed".to_string(),
            message: "stream input timeout".to_string(),
            metadata: HashMap::from([(
                META_GLOBAL_CODE.to_string(),
                (GmvGuardErrorCode::StreamInputTimeout as u16).to_string(),
            )]),
        };

        let error = remote_error(
            "session",
            "start_live",
            error,
            "stream_start_failed",
            "视频流创建失败，请检查设备在线状态和媒体服务",
            false,
        );

        let GuardError::UserVisible {
            code,
            user_message,
            retryable,
            details,
            ..
        } = error
        else {
            panic!("expected user-visible error");
        };
        assert_eq!(code, "stream_input_timeout");
        assert_eq!(
            user_message,
            GmvGuardErrorCode::StreamInputTimeout.out_msg()
        );
        assert!(retryable);
        assert_eq!(
            details.get("remote_code").map(String::as_str),
            Some("session_business_failed")
        );
    }

    #[test]
    fn remote_error_keeps_legacy_detail_code_without_global_metadata() {
        let error = ErrorDetail {
            code: "snapshot_rejected".to_string(),
            message: "device rejected snapshot".to_string(),
            metadata: HashMap::new(),
        };

        let error = remote_error(
            "session",
            "snapshot_image",
            error,
            "snapshot_rejected",
            "抓拍请求未被设备接受，请确认设备在线且支持抓拍",
            true,
        );

        let GuardError::UserVisible {
            code,
            user_message,
            retryable,
            details,
            ..
        } = error
        else {
            panic!("expected user-visible error");
        };
        assert_eq!(code, "snapshot_rejected");
        assert_eq!(user_message, GmvGuardErrorCode::SnapshotRejected.out_msg());
        assert!(retryable);
        assert_eq!(
            details.get("remote_code").map(String::as_str),
            Some("snapshot_rejected")
        );
    }
}
