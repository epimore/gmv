use std::collections::HashMap;

use base::serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::v2::control::{BusinessControl, DeviceStreamOptions};
use crate::auth::AuthState;
use crate::core::{GuardError, GuardResult};
use crate::mqttc::mapping::{CommandAction, RoutedCommand};
use crate::operation::OperationService;
use crate::outbox::OutboxRepository;
use crate::store::InMemoryGuardStore;
use crate::store::model::{INTEGRATION_PLAYBACK_MAX_RENEWALS, INTEGRATION_PLAYBACK_TOKEN_TTL_MS};
use crate::store::model::{OutboxDestinationKind, OutboxRecord, OutboxState};

#[derive(Debug, Clone)]
pub struct MqttCommandExecutor {
    operations: OperationService,
    control: BusinessControl,
    store: InMemoryGuardStore,
    auth: Option<AuthState>,
    result_outbox: Option<(OutboxRepository, HashMap<String, String>)>,
}

impl MqttCommandExecutor {
    pub fn new(operations: OperationService, store: InMemoryGuardStore) -> Self {
        Self {
            operations,
            control: BusinessControl::new(store.clone()),
            store,
            auth: None,
            result_outbox: None,
        }
    }

    pub fn with_auth(mut self, auth: AuthState) -> Self {
        self.auth = Some(auth);
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

    pub async fn execute(&self, command: RoutedCommand) -> GuardResult<()> {
        let requested_by = if command.integration_id.is_empty() {
            "mqtt".to_string()
        } else {
            format!("integration:{}", command.integration_id)
        };
        let operation = self
            .operations
            .start(command.operation_request(requested_by))?;
        let result: GuardResult<()> = async {
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
                        .map(|_| ())
                }
                CommandAction::StreamStop => self
                    .control
                    .stop_stream(&command.command_id, &command.target)
                    .await
                    .map(|_| ()),
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
                        .map(|_| ())
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
                        .map(|_| ())
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
                        .map(|_| ())
                }
                CommandAction::Ptz => {
                    let channel_id = required_payload_string(&command.payload, "channel_id")?;
                    let (ptz_command, speed) = ptz_control(&command.payload)?;
                    self.control
                        .ptz(
                            &command.command_id,
                            &command.target,
                            &channel_id,
                            ptz_command,
                            speed,
                        )
                        .await
                        .map(|_| ())
                }
                CommandAction::AiStart => {
                    let stream_id = payload_string(&command.payload, "stream_id")
                        .unwrap_or_else(|| command.target.clone());
                    let model = required_payload_string(&command.payload, "model")?;
                    self.control
                        .start_ai(&command.command_id, &stream_id, &model)
                        .await
                        .map(|_| ())
                }
                CommandAction::AiCancel => self
                    .control
                    .cancel_ai(&command.command_id, &command.target)
                    .await
                    .map(|_| ()),
                CommandAction::PlaybackTicketRenew => self.renew_playback_ticket(&command).await,
            }
        }
        .await;
        match result {
            Ok(()) => {
                self.operations
                    .succeed(&operation.operation_id, "MQTT command executed")?;
                self.enqueue_result(&command, "succeeded", None).await?;
                Ok(())
            }
            Err(error) => {
                let _ = self.operations.fail(&operation.operation_id, error.clone());
                self.enqueue_result(&command, "failed", Some(error_code(&error)))
                    .await?;
                Err(error)
            }
        }
    }

    async fn renew_playback_ticket(&self, command: &RoutedCommand) -> GuardResult<()> {
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
            return Ok(());
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
        self.store.upsert_playback_ticket(ticket);
        Ok(())
    }

    async fn enqueue_result(
        &self,
        command: &RoutedCommand,
        state: &str,
        error_code: Option<&str>,
    ) -> GuardResult<()> {
        let Some((repository, topics)) = &self.result_outbox else {
            return Ok(());
        };
        let Some(topic) = topics.get(&command.integration_id) else {
            return Ok(());
        };
        let now_ms = now_ms();
        let payload = base::serde_json::to_vec(&base::serde_json::json!({
            "schema_version": "v1",
            "integration_id": command.integration_id,
            "command_id": command.command_id,
            "operation_id": command.command_id,
            "state": state,
            "error_code": error_code,
            "occurred_at_ms": now_ms
        }))
        .map_err(|error| {
            GuardError::InvalidConfig(format!("MQTT result encode failed: {error}"))
        })?;
        let digest = hex::encode(Sha256::digest(command.command_id.as_bytes()));
        repository
            .insert_mapped_outbox_records(vec![OutboxRecord {
                outbox_id: format!("cmd-result-{}", &digest[..32]),
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
            .await
    }
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

fn required_payload_u32(payload: &Value, key: &str) -> GuardResult<u32> {
    payload
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| GuardError::InvalidConfig(format!("MQTT command payload.{key} is required")))
}
