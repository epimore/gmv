use base::serde_json::Value;

use crate::api::v2::control::{BusinessControl, DeviceStreamOptions};
use crate::core::{GuardError, GuardResult};
use crate::mqttc::mapping::{CommandAction, RoutedCommand};
use crate::operation::OperationService;
use crate::store::InMemoryGuardStore;

#[derive(Debug, Clone)]
pub struct MqttCommandExecutor {
    operations: OperationService,
    control: BusinessControl,
}

impl MqttCommandExecutor {
    pub fn new(operations: OperationService, store: InMemoryGuardStore) -> Self {
        Self {
            operations,
            control: BusinessControl::new(store),
        }
    }

    pub async fn execute(&self, command: RoutedCommand) -> GuardResult<()> {
        let operation = self.operations.start(command.operation_request("mqtt"))?;
        let result = match command.action {
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
            CommandAction::StreamTalk => {
                let device_id = payload_string(&command.payload, "device_id")
                    .unwrap_or_else(|| command.target.clone());
                let channel_id = required_payload_string(&command.payload, "channel_id")?;
                self.control
                    .start_talk_with_options(
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
        };
        match result {
            Ok(()) => {
                self.operations
                    .succeed(&operation.operation_id, "MQTT command executed")?;
                Ok(())
            }
            Err(error) => {
                let _ = self.operations.fail(&operation.operation_id, error.clone());
                Err(error)
            }
        }
    }
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
        talk_codec: payload_string(payload, "talk_codec").unwrap_or_default(),
        talk_sample_rate: payload_u32(payload, "talk_sample_rate"),
        talk_channel_count: payload_u32(payload, "talk_channel_count"),
        talk_frame_duration_ms: payload_u32(payload, "talk_frame_duration_ms"),
        playback_id: payload_string(payload, "playback_id").unwrap_or_default(),
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
