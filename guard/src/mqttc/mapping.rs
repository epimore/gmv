use base::serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    StreamStart,
    StreamStop,
    Ptz,
    AiStart,
    AiCancel,
}

impl CommandAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stream.start" => Some(Self::StreamStart),
            "stream.stop" => Some(Self::StreamStop),
            "device.ptz" => Some(Self::Ptz),
            "ai.start" => Some(Self::AiStart),
            "ai.cancel" => Some(Self::AiCancel),
            _ => None,
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
