use std::collections::HashMap;

use base::serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::v2::control::{
    BroadcastOperationOptions, BroadcastTargetOptions, BusinessControl, DeviceStreamOptions,
};
use crate::api::v2::http::{
    GbChannelRequest, GbDeviceRequest, LeaseResponse, NodeResponse, ai_task_summaries,
    cloud_recording_summary, gb_channel_image_response, gb_channel_records_response,
    gb_channel_request, gb_channel_response, gb_device_page_response, gb_device_request,
    gb_device_response, gb_resource_response, gb_session_config_response, stream_summaries,
};
use crate::api::v2::model::MediaTransportCapability;
use crate::api::v2::{ApiV2, EventQuery};
use crate::auth::AuthState;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::mapping::{CommandAction, RoutedCommand};
use crate::operation::{OperationRecord, OperationService};
use crate::outbox::OutboxRepository;
use crate::store::InMemoryGuardStore;
use crate::store::model::{INTEGRATION_PLAYBACK_MAX_RENEWALS, INTEGRATION_PLAYBACK_TOKEN_TTL_MS};
use crate::store::model::{OutboxDestinationKind, OutboxRecord, OutboxState};
use crate::store::persistent::IntegrationRepository;

#[derive(Debug, Clone)]
pub struct MqttCommandExecutor {
    operations: OperationService,
    api: ApiV2,
    control: BusinessControl,
    store: InMemoryGuardStore,
    auth: Option<AuthState>,
    result_outbox: Option<(OutboxRepository, HashMap<String, String>)>,
    result_integrations: Option<IntegrationRepository>,
    media_https_http2_verified: bool,
}

impl MqttCommandExecutor {
    pub fn new(operations: OperationService, store: InMemoryGuardStore) -> Self {
        Self {
            api: ApiV2::new(store.clone(), operations.clone()),
            operations,
            control: BusinessControl::new(store.clone()),
            store,
            auth: None,
            result_outbox: None,
            result_integrations: None,
            media_https_http2_verified: false,
        }
    }

    pub fn with_auth(mut self, auth: AuthState) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn with_media_https_http2_verified(mut self, verified: bool) -> Self {
        self.media_https_http2_verified = verified;
        self
    }

    pub fn with_result_outbox(
        mut self,
        repository: OutboxRepository,
        topics: HashMap<String, String>,
    ) -> Self {
        self.result_outbox = Some((repository, topics));
        self
    }

    pub fn with_dynamic_result_outbox(
        mut self,
        repository: OutboxRepository,
        integrations: IntegrationRepository,
    ) -> Self {
        self.result_outbox = Some((repository, HashMap::new()));
        self.result_integrations = Some(integrations);
        self
    }

    pub async fn execute(&self, command: RoutedCommand) -> GuardResult<()> {
        let requested_by = if command.integration_id.is_empty() {
            "mqtt".to_string()
        } else {
            format!("integration:{}", command.integration_id)
        };
        let operation = self
            .operations
            .start(command.operation_request(requested_by))?;
        let result: GuardResult<Value> = async {
            match command.action {
                CommandAction::StreamStart => {
                    let device_id = payload_string(&command.payload, "device_id")
                        .unwrap_or_else(|| command.target.clone());
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    self.control
                        .start_live_with_options(
                            &command.command_id,
                            &device_id,
                            &channel_id,
                            device_stream_options(&command.payload),
                        )
                        .await
                        .and_then(command_result_value)
                }
                CommandAction::StreamStop => self
                    .control
                    .stop_stream(&command.command_id, &command.target)
                    .await
                    .and_then(command_result_value),
                CommandAction::StreamPlayback => {
                    let device_id = payload_string(&command.payload, "device_id")
                        .unwrap_or_else(|| command.target.clone());
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    self.control
                        .start_playback_with_options(
                            &command.command_id,
                            &device_id,
                            &channel_id,
                            device_stream_options(&command.payload),
                        )
                        .await
                        .and_then(command_result_value)
                }
                CommandAction::StreamDownload => {
                    let device_id = payload_string(&command.payload, "device_id")
                        .unwrap_or_else(|| command.target.clone());
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    self.control
                        .start_download_with_options(
                            &command.command_id,
                            &device_id,
                            &channel_id,
                            device_stream_options(&command.payload),
                        )
                        .await
                        .and_then(command_result_value)
                }
                CommandAction::DeviceBroadcast => {
                    let device_id = payload_string(&command.payload, "device_id")
                        .unwrap_or_else(|| command.target.clone());
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    self.control
                        .start_broadcast_with_options(
                            &command.command_id,
                            &device_id,
                            &channel_id,
                            device_stream_options(&command.payload),
                        )
                        .await
                        .and_then(command_result_value)
                }
                CommandAction::Ptz => {
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    let (ptz_command, speed) = ptz_control(&command.payload)?;
                    let sequence = self
                        .control
                        .ptz(
                            &command.command_id,
                            &command.target,
                            &channel_id,
                            ptz_command,
                            speed,
                        )
                        .await?;
                    Ok(base::serde_json::json!({
                        "accepted": true,
                        "command": ptz_command,
                        "speed": speed,
                        "sequence": sequence,
                        "count": sequence
                    }))
                }
                CommandAction::AiStart => {
                    let stream_id = payload_string(&command.payload, "stream_id")
                        .unwrap_or_else(|| command.target.clone());
                    let model = required_payload_string(&command.payload, "model")?;
                    self.control
                        .start_ai(&command.command_id, &stream_id, &model)
                        .await
                        .and_then(command_result_value)
                }
                CommandAction::AiCancel => self
                    .control
                    .cancel_ai(&command.command_id, &command.target)
                    .await
                    .and_then(command_result_value),
                CommandAction::PlaybackTicketRenew => self.renew_playback_ticket(&command).await,
                CommandAction::Business(action) => {
                    self.execute_business_action(action, &command).await
                }
            }
        }
        .await;
        match result {
            Ok(result) => {
                self.operations
                    .succeed(&operation.operation_id, "MQTT command executed")?;
                self.enqueue_result(&command, "succeeded", None, Some(&result))
                    .await?;
                Ok(())
            }
            Err(error) => {
                if let Err(state_error) =
                    self.operations.fail(&operation.operation_id, error.clone())
                {
                    base::log::warn!(
                        "MQTT command operation finalization failed: action=mqtt_command, stage=terminal, outcome=state_update_failed, command_id={}, reason={}",
                        command.command_id,
                        state_error
                    );
                }
                self.enqueue_result(&command, "failed", Some(error_code(&error)), None)
                    .await?;
                Err(error)
            }
        }
    }

    async fn execute_business_action(
        &self,
        action: &str,
        command: &RoutedCommand,
    ) -> GuardResult<Value> {
        match action {
            "system.dashboard.get" => {
                let events = self.api.poll_events(EventQuery::default())?;
                Ok(base::serde_json::json!({
                    "node_count": self.api.list_nodes().len(),
                    "event_count": events.items.len(),
                    "next_after_id": events.next_after_id
                }))
            }
            "media.transport.get" => {
                command_result_value(MediaTransportCapability::from_https_http2_verified(
                    self.media_https_http2_verified,
                ))
            }
            "media.operation.list" => {
                let requested_by = format!("integration:{}", command.integration_id);
                let ids = command
                    .payload
                    .get("ids")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<std::collections::HashSet<_>>()
                    });
                Ok(Value::Array(
                    self.api
                        .list_operations()
                        .into_iter()
                        .filter(|record| record.checkpoint_ms > 0)
                        .filter(|record| record.requested_by == requested_by)
                        .filter(|record| {
                            ids.as_ref()
                                .is_none_or(|ids| ids.contains(record.operation_id.as_str()))
                        })
                        .map(operation_result_value)
                        .collect(),
                ))
            }
            "media.operation.get" | "media.operation.continue" | "media.operation.cancel" => {
                let operation_id = command_target_or_payload(command, "operation_id")?;
                let record = self.api.get_operation(&operation_id)?;
                require_mqtt_operation_owner(&record, &command.integration_id)?;
                let record = if action == "media.operation.cancel" {
                    self.api.cancel_operation(&operation_id)?
                } else {
                    record
                };
                Ok(operation_result_value(record))
            }
            "node.list" => Ok(Value::Array(
                self.api
                    .list_nodes()
                    .into_iter()
                    .map(NodeResponse::from)
                    .map(|node| base::serde_json::to_value(node).expect("node response serializes"))
                    .collect(),
            )),
            "lease.list" => Ok(Value::Array(
                self.api
                    .list_leases()
                    .into_iter()
                    .map(LeaseResponse::from)
                    .map(|lease| {
                        base::serde_json::to_value(lease).expect("lease response serializes")
                    })
                    .collect(),
            )),
            "device.list" => {
                let devices = self.control.list_gb_devices().await?;
                let mut result = Vec::with_capacity(devices.len());
                for device in devices {
                    let channels = self
                        .control
                        .list_gb_channels(&device.device_id)
                        .await?
                        .into_iter()
                        .map(|channel| channel.channel_id)
                        .collect::<Vec<_>>();
                    result.push(base::serde_json::json!({
                        "device_id": device.device_id,
                        "name": if device.alias.is_empty() { device.domain } else { device.alias },
                        "session_node_id": device.session_node_id,
                        "channels": channels,
                        "online": device.status != 0 && device.del == 0
                    }));
                }
                Ok(Value::Array(result))
            }
            "stream.list" => command_result_value(stream_summaries(&self.store)),
            "ai.list" => command_result_value(ai_task_summaries(&self.store)),
            "runtime.status.get" => {
                let streams = stream_summaries(&self.store);
                let ai_tasks = ai_task_summaries(&self.store);
                Ok(base::serde_json::json!({
                    "guard_available": true,
                    "streams": streams.len(),
                    "running_streams": streams.iter().filter(|item| item.state == crate::api::v2::model::StreamSummaryState::Running).count(),
                    "ai_tasks": ai_tasks.len(),
                    "running_ai_tasks": ai_tasks.iter().filter(|item| item.state == crate::api::v2::model::AiTaskSummaryState::Running).count(),
                    "ptz_commands": 0
                }))
            }
            "stream.release" => {
                let stream_id = command_target_or_payload(command, "stream_id")?;
                let subscription_id = required_payload_string(&command.payload, "subscription_id")?;
                self.control
                    .release_stream(&command.command_id, &stream_id, &subscription_id)
                    .await
                    .and_then(command_result_value)
            }
            "stream.speed.set" => {
                let stream_id = command_target_or_payload(command, "stream_id")?;
                let speed_rate = required_payload_f32(&command.payload, "speed_rate")?;
                self.control
                    .set_playback_speed(&command.command_id, &stream_id, speed_rate)
                    .await?;
                Ok(base::serde_json::json!({"accepted": true, "speed_rate": speed_rate}))
            }
            "stream.output.list" => {
                let stream_id = command_target_or_payload(command, "stream_id")?;
                self.control
                    .list_stream_outputs(&stream_id)
                    .await
                    .and_then(command_result_value)
            }
            "stream.output.create" => {
                let stream_id = command_target_or_payload(command, "stream_id")?;
                let output_type = required_payload_string(&command.payload, "output_type")?;
                let audio_codec =
                    payload_string(&command.payload, "audio_codec").unwrap_or_default();
                let subscription_id = payload_string(&command.payload, "subscription_id")
                    .unwrap_or_else(|| format!("mqtt-{}", command.command_id));
                self.control
                    .validate_stream_output_target(&stream_id, &output_type)?;
                self.control
                    .create_stream_output(
                        &command.command_id,
                        &stream_id,
                        &output_type,
                        &audio_codec,
                        &subscription_id,
                    )
                    .await
                    .and_then(command_result_value)
            }
            "stream.output.close" => {
                let stream_id = required_payload_string(&command.payload, "stream_id")?;
                let output_id = command_target_or_payload(command, "output_id")?;
                let closed = self
                    .control
                    .close_stream_output(&command.command_id, &stream_id, &output_id)
                    .await?;
                Ok(base::serde_json::json!({"closed": closed, "output_id": output_id}))
            }
            "playback.seek" => {
                let playback_id = command_target_or_payload(command, "playback_id")?;
                let stream_id = required_payload_string(&command.payload, "stream_id")?;
                let ticket = self.integration_playback_ticket(command, &playback_id, &stream_id)?;
                let position_sec = required_payload_u32(&command.payload, "position_sec")?;
                if position_sec < ticket.playback_start_time_sec
                    || position_sec > ticket.playback_end_time_sec
                {
                    return Err(GuardError::InvalidConfig(
                        "MQTT playback seek position is outside the selected range".to_string(),
                    ));
                }
                let generation = self
                    .control
                    .seek_playback(
                        &command.command_id,
                        &playback_id,
                        &stream_id,
                        position_sec,
                        payload_u64(&command.payload, "expected_generation"),
                    )
                    .await?;
                Ok(base::serde_json::json!({"accepted": true, "generation": generation}))
            }
            "playback.speed.set" => {
                let playback_id = command_target_or_payload(command, "playback_id")?;
                let stream_id = required_payload_string(&command.payload, "stream_id")?;
                self.integration_playback_ticket(command, &playback_id, &stream_id)?;
                let speed_rate = required_payload_f32(&command.payload, "speed_rate")?;
                let generation = self
                    .control
                    .set_playback_speed_versioned(
                        &command.command_id,
                        &playback_id,
                        &stream_id,
                        speed_rate,
                        payload_u64(&command.payload, "expected_generation"),
                    )
                    .await?;
                Ok(base::serde_json::json!({"accepted": true, "generation": generation}))
            }
            "playback.state.set" => {
                let playback_id = command_target_or_payload(command, "playback_id")?;
                let stream_id = required_payload_string(&command.payload, "stream_id")?;
                let ticket = self.integration_playback_ticket(command, &playback_id, &stream_id)?;
                let paused = required_payload_bool(&command.payload, "paused")?;
                let generation = self
                    .control
                    .set_playback_state(
                        &command.command_id,
                        &playback_id,
                        &stream_id,
                        paused,
                        payload_u64(&command.payload, "expected_generation"),
                        &ticket.subscription_id,
                    )
                    .await?;
                Ok(base::serde_json::json!({"accepted": true, "generation": generation}))
            }
            "broadcast.get" => {
                let broadcast_id = command_target_or_payload(command, "broadcast_id")?;
                self.control
                    .get_broadcast_operation(&broadcast_id)
                    .and_then(command_result_value)
            }
            "broadcast.stop_target" => {
                let broadcast_id = required_payload_string(&command.payload, "broadcast_id")?;
                let leg_id = command_target_or_payload(command, "leg_id")?;
                self.control
                    .stop_broadcast_target(&command.command_id, &broadcast_id, &leg_id)
                    .await
                    .and_then(command_result_value)
            }
            "broadcast.stop_all" => {
                let broadcast_id = command_target_or_payload(command, "broadcast_id")?;
                self.control
                    .stop_broadcast_operation(&command.command_id, &broadcast_id)
                    .await
                    .and_then(command_result_value)
            }
            "broadcast.start" => self.start_broadcast_operation(command).await,
            "gb.session_config.get" => {
                let node_id = command_target_or_payload(command, "node_id")?;
                self.control
                    .gb_session_config(&node_id)
                    .await
                    .map(gb_session_config_response)
                    .and_then(command_result_value)
            }
            "gb.device.list" => {
                let (page, page_size) = mqtt_page(&command.payload, 20, 500)?;
                let session_node_id = payload_string(&command.payload, "session_node_id");
                let domain_id = payload_string(&command.payload, "domain_id");
                let (session_node_id, domain_id) = match (session_node_id, domain_id) {
                    (Some(session), Some(domain)) => (session, domain),
                    (None, None) => self.control.first_gb_session_node_by_domain().await?,
                    _ => {
                        return Err(GuardError::InvalidConfig(
                            "MQTT session_node_id and domain_id must be provided together"
                                .to_string(),
                        ));
                    }
                };
                self.control
                    .list_gb_device_page(
                        &session_node_id,
                        &domain_id,
                        payload_string(&command.payload, "device_id")
                            .as_deref()
                            .unwrap_or_default(),
                        payload_string(&command.payload, "device_name")
                            .as_deref()
                            .unwrap_or_default(),
                        command
                            .payload
                            .get("registered_only")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        None,
                        page,
                        page_size,
                    )
                    .await
                    .map(gb_device_page_response)
                    .and_then(command_result_value)
            }
            "gb.device.create" | "gb.device.update" => {
                let request: GbDeviceRequest =
                    base::serde_json::from_value(command.payload.clone()).map_err(|error| {
                        GuardError::InvalidConfig(format!(
                            "MQTT GB device payload is invalid: {error}"
                        ))
                    })?;
                let device = gb_device_request(request);
                if action == "gb.device.update" && device.device_id != command.target {
                    return Err(GuardError::InvalidConfig(
                        "MQTT GB device payload.device_id must match target".to_string(),
                    ));
                }
                let result = if action == "gb.device.create" {
                    self.control.create_gb_device(device).await
                } else {
                    self.control.update_gb_device(device).await
                }?;
                command_result_value(gb_device_response(result))
            }
            "gb.device.get" => {
                let device_id = command_target_or_payload(command, "device_id")?;
                self.control
                    .get_gb_device(&device_id)
                    .await?
                    .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))
                    .map(gb_device_response)
                    .and_then(command_result_value)
            }
            "gb.device.delete" => {
                let device_id = command_target_or_payload(command, "device_id")?;
                let session_node_id = required_payload_string(&command.payload, "session_node_id")?;
                let domain_id = required_payload_string(&command.payload, "domain_id")?;
                self.control
                    .delete_gb_device(&session_node_id, &device_id, &domain_id)
                    .await?;
                Ok(base::serde_json::json!({"deleted": true, "device_id": device_id}))
            }
            "gb.channel.list" => {
                let device_id = command_target_or_payload(command, "device_id")?;
                let session_node_id = required_payload_string(&command.payload, "session_node_id")?;
                let channels = self
                    .control
                    .list_gb_channels_for_session(&session_node_id, &device_id)
                    .await?
                    .into_iter()
                    .map(gb_channel_response)
                    .collect::<Vec<_>>();
                command_result_value(channels)
            }
            "gb.channel.get" => {
                let device_id = required_payload_string(&command.payload, "device_id")?;
                let channel_id = command_target_or_payload(command, "channel_id")?;
                self.control
                    .get_gb_channel(&device_id, &channel_id)
                    .await?
                    .ok_or_else(|| {
                        GuardError::NotFound(format!("GB28181 channel {device_id}/{channel_id}"))
                    })
                    .map(gb_channel_response)
                    .and_then(command_result_value)
            }
            "gb.channel.update" => {
                let device_id = required_payload_string(&command.payload, "device_id")?;
                let channel_id = command_target_or_payload(command, "channel_id")?;
                let request: GbChannelRequest =
                    base::serde_json::from_value(command.payload.clone()).map_err(|error| {
                        GuardError::InvalidConfig(format!(
                            "MQTT GB channel payload is invalid: {error}"
                        ))
                    })?;
                self.control
                    .update_gb_channel(gb_channel_request(device_id, channel_id, request))
                    .await
                    .map(gb_channel_response)
                    .and_then(command_result_value)
            }
            "gb.resource.list" => {
                let device_id = command_target_or_payload(command, "device_id")?;
                let session_node_id = required_payload_string(&command.payload, "session_node_id")?;
                let resources = self
                    .control
                    .list_gb_resources_for_session(&session_node_id, &device_id)
                    .await?
                    .into_iter()
                    .map(gb_resource_response)
                    .collect::<Vec<_>>();
                command_result_value(resources)
            }
            "gb.resource.confirm" => self.confirm_gb_resource(command, false).await,
            "gb.resource.reset" => self.confirm_gb_resource(command, true).await,
            "gb.image.list" => self.list_gb_images(command).await,
            "gb.image.snapshot" => {
                let device_id = required_payload_string(&command.payload, "device_id")?;
                let channel_id = command_target_or_payload(command, "channel_id")?;
                let session_id = self
                    .control
                    .snapshot_image(
                        &command.command_id,
                        &device_id,
                        &channel_id,
                        payload_u32(&command.payload, "count"),
                        payload_u32(&command.payload, "interval"),
                    )
                    .await?;
                Ok(base::serde_json::json!({"session_id": session_id}))
            }
            "gb.image.access" => self.issue_gb_image_access(command).await,
            "gb.image.cover" => self.set_gb_image_cover(command).await,
            "gb.record.list" => self.list_gb_records(command, false).await,
            "gb.record.query" => self.list_gb_records(command, true).await,
            "cloud_recording.list" => self.list_cloud_recordings(command).await,
            "cloud_recording.create" => self.create_cloud_recording(command).await,
            "cloud_recording.get" => {
                let task_id = command_target_or_payload(command, "task_id")?;
                self.control
                    .get_cloud_recording(&task_id)
                    .await
                    .map(cloud_recording_summary)
                    .and_then(command_result_value)
            }
            "cloud_recording.stop" | "cloud_recording.delete" => {
                let task_id = command_target_or_payload(command, "task_id")?;
                let result = if action == "cloud_recording.stop" {
                    self.control
                        .stop_cloud_recording(&task_id, &command.command_id)
                        .await
                } else {
                    self.control
                        .delete_cloud_recording(&task_id, &command.command_id)
                        .await
                }?;
                command_result_value(cloud_recording_summary(result))
            }
            "cloud_recording.access" => {
                let task_id = command_target_or_payload(command, "task_id")?;
                let mode = payload_string(&command.payload, "mode")
                    .unwrap_or_else(|| "inline".to_string());
                let access = self
                    .control
                    .issue_cloud_recording_access(&task_id, &command.command_id, &mode)
                    .await?;
                Ok(base::serde_json::json!({
                    "url": access.url,
                    "expires_at_ms": access.expires_at_ms,
                    "content_type": access.content_type,
                    "file_name": access.file_name,
                    "file_size": access.file_size
                }))
            }
            "gb.stream.list" => self.list_gb_streams(command).await,
            "gb.stream.management" => self.get_gb_stream_management(command).await,
            "gb.stream.history" => self.list_gb_stream_history(command).await,
            "gb.stream.stop" => self.stop_gb_stream(command).await,
            "playback.presence.heartbeat" => self.heartbeat_playback_presence(command).await,
            _ => Err(GuardError::InvalidConfig(format!(
                "MQTT command action {action} is catalogued but has no executor"
            ))),
        }
    }

    fn integration_playback_ticket(
        &self,
        command: &RoutedCommand,
        playback_id: &str,
        stream_id: &str,
    ) -> GuardResult<crate::store::model::PlaybackTicketRecord> {
        self.store
            .find_playback_control_ticket(playback_id, stream_id)
            .filter(|ticket| ticket.username == format!("integration:{}", command.integration_id))
            .ok_or_else(|| {
                GuardError::InvalidIdentity("playback control owner not found".to_string())
            })
    }

    async fn confirm_gb_resource(
        &self,
        command: &RoutedCommand,
        reset: bool,
    ) -> GuardResult<Value> {
        let device_id = required_payload_string(&command.payload, "device_id")?;
        let resource_id = command_target_or_payload(command, "resource_id")?;
        let confirmed_by = format!("integration:{}", command.integration_id);
        let resource = if reset {
            self.control
                .reset_gb_resource_confirmation(
                    gmv_protocol::session::v1::ResetGbResourceConfirmationRequest {
                        device_id,
                        resource_id,
                        confirmed_by,
                        request_id: command.command_id.clone(),
                    },
                )
                .await?
        } else {
            let current = self
                .control
                .list_gb_resources(&device_id)
                .await?
                .into_iter()
                .find(|resource| resource.resource_id == resource_id)
                .ok_or_else(|| GuardError::NotFound("GB28181 resource".to_string()))?;
            self.control
                .save_gb_resource_confirmation(
                    gmv_protocol::session::v1::SaveGbResourceConfirmationRequest {
                        device_id,
                        resource_id,
                        resource_kind: required_payload_string(&command.payload, "resource_kind")?,
                        owner_scope: required_payload_string(&command.payload, "owner_scope")?,
                        owner_id: required_payload_string(&command.payload, "owner_id")?,
                        suggested_enum_id: current.enum_id,
                        source_parent_id: current.parent_id,
                        confirmed_by,
                        remark: payload_string(&command.payload, "remark").unwrap_or_default(),
                        request_id: command.command_id.clone(),
                    },
                )
                .await?
        };
        command_result_value(gb_resource_response(resource))
    }

    async fn list_gb_images(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let (page, page_size) = mqtt_page(&command.payload, 12, 100)?;
        let device_id = required_payload_string(&command.payload, "device_id")?;
        let channel_id = command_target_or_payload(command, "channel_id")?;
        let session_node_id = match payload_string(&command.payload, "session_node_id") {
            Some(value) => value,
            None => self
                .control
                .get_gb_device(&device_id)
                .await?
                .map(|device| device.session_node_id)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?,
        };
        let response = self
            .control
            .list_gb_channel_images(
                &session_node_id,
                &device_id,
                &channel_id,
                payload_i64(&command.payload, "start_time_ms"),
                payload_i64(&command.payload, "end_time_ms"),
                page,
                page_size,
            )
            .await?;
        let items = response
            .images
            .into_iter()
            .map(gb_channel_image_response)
            .collect::<Vec<_>>();
        Ok(base::serde_json::json!({
            "items": items,
            "total": response.total,
            "page": response.page,
            "page_size": response.page_size
        }))
    }

    async fn issue_gb_image_access(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let image_id = command_target_or_payload(command, "image_id")?;
        let device_id = required_payload_string(&command.payload, "device_id")?;
        let channel_id = required_payload_string(&command.payload, "channel_id")?;
        let session_node_id = required_payload_string(&command.payload, "session_node_id")?;
        let access = self
            .control
            .issue_gb_channel_image_access(
                &session_node_id,
                gmv_protocol::session::v1::IssueGbChannelImageAccessRequest {
                    operation: Some(gmv_protocol::common::v1::OperationRef {
                        operation_id: command.command_id.clone(),
                        idempotency_key: command.command_id.clone(),
                    }),
                    image_id,
                    device_id,
                    channel_id,
                    mode: payload_string(&command.payload, "mode")
                        .unwrap_or_else(|| "inline".to_string()),
                },
            )
            .await?;
        Ok(base::serde_json::json!({
            "url": access.url,
            "expires_at_ms": access.expires_at_ms,
            "content_type": access.content_type,
            "file_name": access.file_name,
            "file_size": access.file_size
        }))
    }

    async fn set_gb_image_cover(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let image_id = command_target_or_payload(command, "image_id")?;
        let response = self
            .control
            .set_gb_channel_cover(
                &required_payload_string(&command.payload, "session_node_id")?,
                gmv_protocol::session::v1::SetGbChannelCoverRequest {
                    operation: Some(gmv_protocol::common::v1::OperationRef {
                        operation_id: command.command_id.clone(),
                        idempotency_key: command.command_id.clone(),
                    }),
                    device_id: required_payload_string(&command.payload, "device_id")?,
                    channel_id: required_payload_string(&command.payload, "channel_id")?,
                    image_id,
                },
            )
            .await?;
        response
            .channel
            .ok_or_else(|| {
                GuardError::Conflict("session returned empty GB28181 channel".to_string())
            })
            .map(gb_channel_response)
            .and_then(command_result_value)
    }

    async fn list_gb_records(&self, command: &RoutedCommand, refresh: bool) -> GuardResult<Value> {
        let device_id = required_payload_string(&command.payload, "device_id")?;
        let channel_id = command_target_or_payload(command, "channel_id")?;
        let session_node_id = required_payload_string(&command.payload, "session_node_id")?;
        let start_time_sec = payload_i64(&command.payload, "start_time_sec");
        let end_time_sec = payload_i64(&command.payload, "end_time_sec");
        let records = if refresh {
            if start_time_sec <= 0 || end_time_sec <= start_time_sec {
                return Err(GuardError::InvalidConfig(
                    "MQTT record query requires a valid time range".to_string(),
                ));
            }
            self.control
                .query_gb_channel_records(
                    &session_node_id,
                    &command.command_id,
                    &device_id,
                    &channel_id,
                    start_time_sec,
                    end_time_sec,
                )
                .await?
        } else {
            let (page, page_size) = mqtt_page(&command.payload, 10, 100)?;
            self.control
                .get_gb_channel_records(
                    &session_node_id,
                    &device_id,
                    &channel_id,
                    start_time_sec,
                    end_time_sec,
                    page,
                    page_size,
                )
                .await?
        };
        command_result_value(gb_channel_records_response(records))
    }

    async fn list_cloud_recordings(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let (page, page_size) = mqtt_page(&command.payload, 50, 100)?;
        let response = self
            .control
            .list_cloud_recordings(
                &required_payload_string(&command.payload, "session_node_id")?,
                gmv_protocol::session::v1::ListCloudRecordingsRequest {
                    device_id: required_payload_string(&command.payload, "device_id")?,
                    channel_id: command_target_or_payload(command, "channel_id")?,
                    page,
                    page_size,
                    include_deleted: false,
                },
            )
            .await?;
        let items = response
            .0
            .into_iter()
            .map(cloud_recording_summary)
            .collect::<Vec<_>>();
        Ok(base::serde_json::json!({
            "items": items,
            "total": response.1,
            "page": response.2,
            "page_size": response.3
        }))
    }

    async fn create_cloud_recording(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let channel_id = command_target_or_payload(command, "channel_id")?;
        self.control
            .create_cloud_recording(gmv_protocol::session::v1::CreateCloudRecordingRequest {
                operation: Some(gmv_protocol::common::v1::OperationRef {
                    operation_id: command.command_id.clone(),
                    idempotency_key: command.command_id.clone(),
                }),
                request_id: command.command_id.clone(),
                session_node_id: required_payload_string(&command.payload, "session_node_id")?,
                device_id: required_payload_string(&command.payload, "device_id")?,
                channel_id,
                start_time_sec: payload_i64(&command.payload, "start_time_sec"),
                end_time_sec: payload_i64(&command.payload, "end_time_sec"),
                requested_by: format!("integration:{}", command.integration_id),
            })
            .await
            .map(cloud_recording_summary)
            .and_then(command_result_value)
    }

    async fn list_gb_streams(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let response = self
            .control
            .list_active_stream_dialogs(
                &required_payload_string(&command.payload, "session_node_id")?,
                gmv_protocol::session::v1::ListActiveStreamDialogsRequest {
                    page: payload_u32(&command.payload, "page"),
                    page_size: payload_u32(&command.payload, "page_size"),
                    stream_id: payload_string(&command.payload, "stream_id").unwrap_or_default(),
                    stream_node_id: payload_string(&command.payload, "stream_node_id")
                        .unwrap_or_default(),
                    device_id: payload_string(&command.payload, "device_id").unwrap_or_default(),
                    channel_id: payload_string(&command.payload, "channel_id").unwrap_or_default(),
                    ssrc: payload_string(&command.payload, "ssrc").unwrap_or_default(),
                    dialog_state: payload_string(&command.payload, "dialog_state")
                        .unwrap_or_default(),
                    expected_session: None,
                },
            )
            .await?;
        let items = response
            .items
            .into_iter()
            .map(active_stream_dialog_value)
            .collect::<Vec<_>>();
        Ok(base::serde_json::json!({
            "items": items,
            "total": response.total,
            "page": response.page,
            "page_size": response.page_size,
            "server_time_ms": response.server_time_ms
        }))
    }

    async fn get_gb_stream_management(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let stream_id = command_target_or_payload(command, "stream_id")?;
        let response = self
            .control
            .get_active_stream_management(
                &required_payload_string(&command.payload, "session_node_id")?,
                &stream_id,
            )
            .await?;
        match gmv_protocol::session::v1::ActiveStreamManagementState::try_from(response.state) {
            Ok(gmv_protocol::session::v1::ActiveStreamManagementState::Active) => {
                let active = response.active.ok_or_else(|| {
                    GuardError::Conflict(
                        "session omitted active stream management item".to_string(),
                    )
                })?;
                Ok(base::serde_json::json!({
                    "state": "active",
                    "active": active_stream_value(active),
                    "ended": null
                }))
            }
            Ok(gmv_protocol::session::v1::ActiveStreamManagementState::Ended) => {
                let ended = response.ended.ok_or_else(|| {
                    GuardError::Conflict("session omitted ended stream management item".to_string())
                })?;
                Ok(base::serde_json::json!({
                    "state": "ended",
                    "active": null,
                    "ended": stream_history_value(ended)
                }))
            }
            _ => Err(GuardError::Conflict(
                "session returned invalid stream management state".to_string(),
            )),
        }
    }

    async fn list_gb_stream_history(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let response = self
            .control
            .list_stream_history(
                &required_payload_string(&command.payload, "session_node_id")?,
                gmv_protocol::session::v1::ListStreamHistoryRequest {
                    page: payload_u32(&command.payload, "page"),
                    page_size: payload_u32(&command.payload, "page_size"),
                    stream_id: payload_string(&command.payload, "stream_id").unwrap_or_default(),
                    stream_node_id: payload_string(&command.payload, "stream_node_id")
                        .unwrap_or_default(),
                    device_id: payload_string(&command.payload, "device_id").unwrap_or_default(),
                    channel_id: payload_string(&command.payload, "channel_id").unwrap_or_default(),
                    ssrc: payload_string(&command.payload, "ssrc").unwrap_or_default(),
                    state: payload_string(&command.payload, "state").unwrap_or_default(),
                    expected_session: None,
                },
            )
            .await?;
        let items = response
            .items
            .into_iter()
            .map(stream_history_value)
            .collect::<Vec<_>>();
        Ok(base::serde_json::json!({
            "items": items,
            "total": response.total,
            "page": response.page,
            "page_size": response.page_size,
            "server_time_ms": response.server_time_ms
        }))
    }

    async fn stop_gb_stream(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let stream_id = command_target_or_payload(command, "stream_id")?;
        let response = self
            .control
            .stop_monitored_stream(
                &required_payload_string(&command.payload, "session_node_id")?,
                &command.command_id,
                &stream_id,
                &required_payload_string(&command.payload, "stop_reason")?,
            )
            .await?;
        let state = match gmv_protocol::session::v1::DeviceStreamState::try_from(response.state) {
            Ok(gmv_protocol::session::v1::DeviceStreamState::Stopping) => "stopping",
            Ok(gmv_protocol::session::v1::DeviceStreamState::Stopped) => "stopped",
            _ => "unknown",
        };
        Ok(base::serde_json::json!({
            "stream_id": response.stream_id,
            "state": state,
            "session_node_id": response.session_node_id,
            "session_instance_id": response.session_instance_id
        }))
    }

    async fn heartbeat_playback_presence(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let items = command
            .payload
            .get("items")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty() && items.len() <= 64)
            .ok_or_else(|| {
                GuardError::InvalidConfig(
                    "MQTT playback presence items must contain 1..=64 entries".to_string(),
                )
            })?;
        let mut rpc_items = Vec::with_capacity(items.len());
        for item in items {
            let playback_id = required_payload_string(item, "playback_id")?;
            let stream_id = required_payload_string(item, "stream_id")?;
            let subscription_id = required_payload_string(item, "subscription_id")?;
            let ticket = self.integration_playback_ticket(command, &playback_id, &stream_id)?;
            if ticket.subscription_id != subscription_id {
                return Err(GuardError::InvalidIdentity(
                    "playback presence subscription owner mismatch".to_string(),
                ));
            }
            rpc_items.push(gmv_protocol::session::v1::PlaybackPresenceHeartbeat {
                playback_id,
                stream_id,
                subscription_id,
                generation: payload_u64(item, "generation"),
            });
        }
        let (server_time_ms, results) = self.control.refresh_playback_presences(rpc_items).await?;
        Ok(base::serde_json::json!({
            "server_time_ms": server_time_ms,
            "items": results.into_iter().map(|item| base::serde_json::json!({
                "playback_id": item.playback_id,
                "stream_id": item.stream_id,
                "accepted": item.accepted,
                "terminal": item.terminal,
                "generation": item.generation,
                "presence_deadline_ms": item.presence_deadline_ms
            })).collect::<Vec<_>>()
        }))
    }

    async fn start_broadcast_operation(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let codec = payload_string(&command.payload, "codec").unwrap_or_else(|| "PCMA".to_string());
        let sample_rate = command
            .payload
            .get("sample_rate")
            .map(|_| payload_u32(&command.payload, "sample_rate"))
            .unwrap_or(8_000);
        let channel_count = command
            .payload
            .get("channel_count")
            .map(|_| payload_u32(&command.payload, "channel_count"))
            .unwrap_or(1);
        let frame_duration_ms = command
            .payload
            .get("frame_duration_ms")
            .map(|_| payload_u32(&command.payload, "frame_duration_ms"))
            .unwrap_or(20);
        if codec != "PCMA" || sample_rate != 8_000 || channel_count != 1 || frame_duration_ms != 20
        {
            return Err(GuardError::InvalidConfig(
                "broadcast_profile_unsupported: expected PCMA/8000/mono/20ms".to_string(),
            ));
        }
        let targets = command
            .payload
            .get("targets")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GuardError::InvalidConfig("MQTT command payload.targets is required".to_string())
            })?
            .iter()
            .map(|target| {
                Ok(BroadcastTargetOptions {
                    device_id: required_payload_string(target, "device_id")?,
                    channel_id: required_payload_string(target, "channel_id")?,
                    session_node_id: payload_string(target, "session_node_id").unwrap_or_default(),
                    trans_mode: payload_string(target, "trans_mode").unwrap_or_default(),
                })
            })
            .collect::<GuardResult<Vec<_>>>()?;
        self.control
            .start_broadcast_operation(
                &command.command_id,
                BroadcastOperationOptions {
                    token: payload_string(&command.payload, "token").unwrap_or_default(),
                    default_trans_mode: payload_string(&command.payload, "default_trans_mode")
                        .unwrap_or_else(|| "udp".to_string()),
                    codec,
                    sample_rate,
                    channel_count,
                    frame_duration_ms,
                    targets,
                },
            )
            .await
            .and_then(command_result_value)
    }

    async fn renew_playback_ticket(&self, command: &RoutedCommand) -> GuardResult<Value> {
        let mut ticket = self
            .store
            .get_playback_ticket(&command.target)
            .ok_or_else(|| GuardError::NotFound("playback ticket".to_string()))?;
        if ticket.username != format!("integration:{}", command.integration_id) {
            return Err(GuardError::InvalidIdentity(
                "playback ticket owner mismatch".to_string(),
            ));
        }
        let renew = command
            .payload
            .get("renew")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                GuardError::InvalidConfig("MQTT command payload.renew is required".to_string())
            })?;
        if !renew {
            self.store.revoke_playback_token(&ticket.token);
            return Ok(base::serde_json::json!({
                "renewed": false,
                "revoked": true,
                "expires_at_ms": null
            }));
        }
        let now_ms = now_ms();
        if ticket.expires_at_ms <= now_ms {
            self.store.revoke_playback_token(&ticket.token);
            return Err(GuardError::InvalidIdentity(
                "playback ticket expired".to_string(),
            ));
        }
        let auth = self.auth.as_ref().ok_or_else(|| {
            GuardError::InvalidConfig("MQTT integration auth state is missing".to_string())
        })?;
        if ticket.renewal_count >= INTEGRATION_PLAYBACK_MAX_RENEWALS
            || now_ms.saturating_add(INTEGRATION_PLAYBACK_TOKEN_TTL_MS)
                > ticket.absolute_expires_at_ms
        {
            self.store.revoke_playback_token(&ticket.token);
            return Err(GuardError::InvalidIdentity(
                "playback ticket renewal limit reached".to_string(),
            ));
        }
        ticket.expires_at_ms = now_ms.saturating_add(INTEGRATION_PLAYBACK_TOKEN_TTL_MS);
        ticket.renewal_count = ticket.renewal_count.saturating_add(1);
        auth.extend_service_session(
            &ticket.ui_session_token,
            std::time::Duration::from_millis(INTEGRATION_PLAYBACK_TOKEN_TTL_MS as u64),
        )?;
        let expires_at_ms = ticket.expires_at_ms;
        self.store.upsert_playback_ticket(ticket);
        Ok(base::serde_json::json!({
            "renewed": true,
            "revoked": false,
            "expires_at_ms": expires_at_ms
        }))
    }

    async fn enqueue_result(
        &self,
        command: &RoutedCommand,
        state: &str,
        error_code: Option<&str>,
        result: Option<&Value>,
    ) -> GuardResult<()> {
        let Some((repository, topics)) = &self.result_outbox else {
            return Ok(());
        };
        let topic = if let Some(integrations) = &self.result_integrations {
            integrations
                .mqtt_config(&command.integration_id)
                .await?
                .map(|config| config.result_topic)
        } else {
            topics.get(&command.integration_id).cloned()
        };
        let Some(topic) = topic else { return Ok(()) };
        let now_ms = now_ms();
        let payload = base::serde_json::to_vec(&base::serde_json::json!({
            "schema_version": "v1",
            "integration_id": command.integration_id,
            "command_id": command.command_id,
            "operation_id": command.command_id,
            "action": command.action.as_str(),
            "state": state,
            "error_code": error_code,
            "result": result,
            "occurred_at_ms": now_ms
        }))
        .map_err(|error| {
            GuardError::InvalidConfig(format!("MQTT result encode failed: {error}"))
        })?;
        let digest = hex::encode(Sha256::digest(command.command_id.as_bytes()));
        let outbox_id = format!("cmd-result-{}", &digest[..32]);
        let payload_bytes = payload.len();
        repository
            .insert_mapped_outbox_records(vec![OutboxRecord {
                outbox_id: outbox_id.clone(),
                event_id: command.command_id.clone(),
                integration_id: command.integration_id.clone(),
                mapping_id: format!("mqtt-command-result:{}", command.integration_id),
                destination_kind: OutboxDestinationKind::Mqtt,
                destination: topic.clone(),
                payload,
                state: OutboxState::Pending,
                attempts: 0,
                next_attempt_at_ms: now_ms,
                last_error: None,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: Some(command.expires_at_ms),
            }])
            .await?;
        base::log::info!(
            "MQTT command result queued: action=mqtt_command_result, stage=outbox, outcome=queued, command_id={}, integration_id={}, outbox_id={}, state={}, topic={}, payload_bytes={}",
            command.command_id,
            command.integration_id,
            outbox_id,
            state,
            topic,
            payload_bytes
        );
        Ok(())
    }
}

fn command_result_value<T: base::serde::Serialize>(value: T) -> GuardResult<Value> {
    base::serde_json::to_value(value).map_err(|error| {
        GuardError::InvalidConfig(format!("MQTT command result encode failed: {error}"))
    })
}

fn command_target_or_payload(command: &RoutedCommand, key: &str) -> GuardResult<String> {
    payload_string(&command.payload, key)
        .or_else(|| (!command.target.trim().is_empty()).then(|| command.target.clone()))
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command {key} is required")))
}

fn operation_result_value(record: OperationRecord) -> Value {
    let status = match record.status {
        crate::operation::OperationStatus::Accepted => "accepted",
        crate::operation::OperationStatus::Running => "running",
        crate::operation::OperationStatus::Succeeded => "succeeded",
        crate::operation::OperationStatus::Failed => "failed",
        crate::operation::OperationStatus::Cancelled => "cancelled",
    };
    base::serde_json::json!({
        "operation_id": record.operation_id,
        "kind": record.kind,
        "state": status,
        "progress_percent": record.progress_percent,
        "stage": record.stage,
        "message": record.message,
        "result": record.result,
        "error": record.error.map(|error| error.to_string()),
        "started_at_ms": record.started_at_ms,
        "updated_at_ms": record.updated_at_ms,
        "checkpoint_ms": record.checkpoint_ms,
        "hard_timeout_ms": record.hard_timeout_ms
    })
}

fn require_mqtt_operation_owner(record: &OperationRecord, integration_id: &str) -> GuardResult<()> {
    if record.requested_by == format!("integration:{integration_id}") {
        Ok(())
    } else {
        Err(GuardError::InvalidIdentity(
            "media operation belongs to another integration".to_string(),
        ))
    }
}

fn active_stream_dialog_value(item: gmv_protocol::session::v1::ActiveStreamDialogItem) -> Value {
    base::serde_json::json!({
        "stream_id": item.stream_id,
        "session_node_id": item.session_node_id,
        "session_instance_id": item.session_instance_id,
        "stream_node_id": item.stream_node_id,
        "device_id": item.device_id,
        "channel_id": item.channel_id,
        "ssrc": item.ssrc,
        "dialog_state": item.dialog_state,
        "created_at_ms": item.created_at_ms,
        "established_at_ms": item.established_at_ms,
        "started_at_ms": item.started_at_ms,
        "session_type": item.session_type
    })
}

fn active_stream_value(item: gmv_protocol::session::v1::ActiveStreamItem) -> Value {
    base::serde_json::json!({
        "stream_id": item.stream_id,
        "session_node_id": item.session_node_id,
        "session_instance_id": item.session_instance_id,
        "stream_node_id": item.stream_node_id,
        "device_id": item.device_id,
        "channel_id": item.channel_id,
        "ssrc": item.ssrc,
        "state": item.state,
        "dialog_state": item.dialog_state,
        "media_state": item.media_state,
        "media_ready": item.media_ready,
        "created_at_ms": item.created_at_ms,
        "established_at_ms": item.established_at_ms,
        "started_at_ms": item.started_at_ms,
        "diagnostic_reason": item.diagnostic_reason,
        "session_type": item.session_type,
        "viewer_count": item.viewer_count,
        "viewer_formats": item.viewer_formats.into_iter().map(|format| base::serde_json::json!({
            "media_format": format.media_format,
            "viewer_count": format.viewer_count
        })).collect::<Vec<_>>(),
        "supported_formats": item.supported_formats,
        "output_format": item.output_format,
        "requested_stream_profile": item.requested_stream_profile,
        "effective_stream_profile": item.effective_stream_profile,
        "stream_profile_verification": item.stream_profile_verification
    })
}

fn stream_history_value(item: gmv_protocol::session::v1::StreamHistoryItem) -> Value {
    base::serde_json::json!({
        "stream_id": item.stream_id,
        "session_node_id": item.session_node_id,
        "stream_node_id": item.stream_node_id,
        "device_id": item.device_id,
        "channel_id": item.channel_id,
        "ssrc": item.ssrc,
        "session_type": item.session_type,
        "state": item.state,
        "created_at_ms": item.created_at_ms,
        "established_at_ms": item.established_at_ms,
        "terminated_at_ms": item.terminated_at_ms,
        "duration_ms": item.duration_ms,
        "terminal_reason": item.terminal_reason,
        "terminal_reason_label": item.terminal_reason_label,
        "error_code": item.error_code,
        "legacy_terminal_time": item.legacy_terminal_time,
        "stop_reason": item.stop_reason
    })
}

fn error_code(error: &GuardError) -> &'static str {
    match error {
        GuardError::InvalidConfig(_) => "invalid_command",
        GuardError::InvalidIdentity(_) => "forbidden",
        GuardError::Conflict(_) => "conflict",
        GuardError::NotFound(_) => "not_found",
        GuardError::StaleInstance(_) => "stale_instance",
        GuardError::Capacity(_) => "capacity_exceeded",
        GuardError::TimeUnsynced(_) => "time_unsynced",
        GuardError::DuplicateEvent(_) => "duplicate",
        GuardError::UserVisible { .. } => "business_error",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

fn required_payload_string(payload: &Value, key: &str) -> GuardResult<String> {
    payload_string(payload, key)
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command payload.{key} is required")))
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn device_stream_options(payload: &Value) -> DeviceStreamOptions {
    DeviceStreamOptions {
        session_node_id: payload_string(payload, "session_node_id").unwrap_or_default(),
        token: payload_string(payload, "token").unwrap_or_default(),
        start_time_sec: payload_u32(payload, "start_time_sec"),
        end_time_sec: payload_u32(payload, "end_time_sec"),
        trans_mode: payload_string(payload, "trans_mode").unwrap_or_default(),
        output_type: payload_string(payload, "output_type").unwrap_or_default(),
        audio_codec: payload_string(payload, "audio_codec").unwrap_or_default(),
        broadcast_codec: payload_string(payload, "broadcast_codec").unwrap_or_default(),
        broadcast_sample_rate: payload_u32(payload, "broadcast_sample_rate"),
        broadcast_channel_count: payload_u32(payload, "broadcast_channel_count"),
        broadcast_frame_duration_ms: payload_u32(payload, "broadcast_frame_duration_ms"),
        playback_id: payload_string(payload, "playback_id").unwrap_or_default(),
        broadcast_id: String::new(),
        broadcast_leg_id: String::new(),
        expected_stream_node_id: String::new(),
        stream_profile: payload_string(payload, "stream_profile").unwrap_or_default(),
    }
}

fn ptz_control(payload: &Value) -> GuardResult<(&'static str, u32)> {
    let left_right = required_payload_u32(payload, "leftRight")?;
    let up_down = required_payload_u32(payload, "upDown")?;
    let in_out = required_payload_u32(payload, "inOut")?;
    let horizon_speed = required_payload_u32(payload, "horizonSpeed")?;
    let vertical_speed = required_payload_u32(payload, "verticalSpeed")?;
    let zoom_speed = required_payload_u32(payload, "zoomSpeed")?;
    let command = match (left_right, up_down, in_out) {
        (0, 0, 0) => "stop",
        (1, 1, 0) => "left_up",
        (2, 1, 0) => "right_up",
        (1, 2, 0) => "left_down",
        (2, 2, 0) => "right_down",
        (1, 0, 0) => "left",
        (2, 0, 0) => "right",
        (0, 1, 0) => "up",
        (0, 2, 0) => "down",
        (0, 0, 1) => "zoom_out",
        (0, 0, 2) => "zoom_in",
        _ => {
            return Err(GuardError::InvalidConfig(
                "MQTT command payload ptz control values are invalid".to_string(),
            ));
        }
    };
    let speed = if command == "stop" {
        1
    } else if in_out > 0 && zoom_speed > 0 {
        zoom_speed
    } else {
        let mut speed = 0;
        if left_right > 0 {
            speed = speed.max(horizon_speed);
        }
        if up_down > 0 {
            speed = speed.max(vertical_speed);
        }
        if speed == 0 {
            return Err(GuardError::InvalidConfig(
                "MQTT command payload ptz speed is required".to_string(),
            ));
        }
        speed
    };
    Ok((command, speed))
}

fn payload_u32(payload: &Value, key: &str) -> u32 {
    payload
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default()
}

fn mqtt_page(
    payload: &Value,
    default_page_size: u32,
    max_page_size: u32,
) -> GuardResult<(u32, u32)> {
    let page = optional_payload_u32(payload, "page")?.unwrap_or(1);
    let page_size = optional_payload_u32(payload, "page_size")?.unwrap_or(default_page_size);
    if page == 0 || !(1..=max_page_size).contains(&page_size) {
        return Err(GuardError::InvalidConfig(format!(
            "MQTT page must be positive and page_size must be between 1 and {max_page_size}"
        )));
    }
    Ok((page, page_size))
}

fn optional_payload_u32(payload: &Value, key: &str) -> GuardResult<Option<u32>> {
    let Some(value) = payload.get(key) else {
        return Ok(None);
    };
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .map(Some)
        .ok_or_else(|| {
            GuardError::InvalidConfig(format!(
                "MQTT command payload.{key} must be an unsigned 32-bit integer"
            ))
        })
}

fn payload_u64(payload: &Value, key: &str) -> u64 {
    payload.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn payload_i64(payload: &Value, key: &str) -> i64 {
    payload.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn required_payload_f32(payload: &Value, key: &str) -> GuardResult<f32> {
    payload
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .map(|value| value as f32)
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command payload.{key} is required")))
}

fn required_payload_bool(payload: &Value, key: &str) -> GuardResult<bool> {
    payload
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command payload.{key} is required")))
}

fn required_payload_u32(payload: &Value, key: &str) -> GuardResult<u32> {
    payload
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command payload.{key} is required")))
}
