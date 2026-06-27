use base::serde_json::Value;

use crate::auth::Role;
use crate::operation::OperationRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    StreamStart,
    StreamStop,
    StreamPlayback,
    StreamDownload,
    StreamTalk,
    Ptz,
    AiStart,
    AiCancel,
}

impl CommandAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stream.start" => Some(Self::StreamStart),
            "stream.stop" => Some(Self::StreamStop),
            "stream.playback" => Some(Self::StreamPlayback),
            "stream.download" => Some(Self::StreamDownload),
            "device.talk" => Some(Self::StreamTalk),
            "device.ptz" => Some(Self::Ptz),
            "ai.start" => Some(Self::AiStart),
            "ai.cancel" => Some(Self::AiCancel),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::StreamStart => "stream.start",
            Self::StreamStop => "stream.stop",
            Self::StreamPlayback => "stream.playback",
            Self::StreamDownload => "stream.download",
            Self::StreamTalk => "device.talk",
            Self::Ptz => "device.ptz",
            Self::AiStart => "ai.start",
            Self::AiCancel => "ai.cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedCommand {
    pub command_id: String,
    pub action: CommandAction,
    pub target: String,
    pub payload: Value,
}

impl RoutedCommand {
    pub fn operation_request(&self, requested_by: impl Into<String>) -> OperationRequest {
        OperationRequest {
            operation_id: self.command_id.clone(),
            kind: self.action.as_str().to_string(),
            requested_by: requested_by.into(),
            caller_role: Role::Operator,
            required_role: Role::Operator,
            dangerous: false,
            confirmation: None,
        }
    }
}
