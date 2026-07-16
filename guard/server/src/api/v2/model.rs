#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct DeviceSummary {
    pub device_id: String,
    pub name: String,
    pub session_node_id: String,
    pub channels: Vec<String>,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum StreamSummaryState {
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
pub struct StreamSummary {
    pub stream_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub endpoint: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub subscription_id: String,
    pub session_node_id: String,
    pub session_instance_id: String,
    pub playback_id: String,
    pub playback_generation: u64,
    pub playback_start_time_sec: u32,
    pub playback_end_time_sec: u32,
    pub state: StreamSummaryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum StreamOutputState {
    Preparing,
    Ready,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct StreamOutputSummary {
    pub output_id: String,
    pub stream_id: String,
    pub output_type: String,
    pub endpoint: String,
    pub state: StreamOutputState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum MediaOperationState {
    Preparing,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct MediaOperationError {
    pub code: String,
    pub message: String,
    pub user_message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct MediaOperationSummary {
    pub operation_id: String,
    pub state: MediaOperationState,
    pub stage: String,
    pub elapsed_ms: u64,
    pub last_progress_at_ms: i64,
    pub checkpoint_ms: u64,
    pub hard_timeout_ms: u64,
    pub can_continue: bool,
    pub result: Option<base::serde_json::Value>,
    pub error: Option<MediaOperationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct MediaTransportCapability {
    pub scheme: String,
    pub http_version: String,
    pub multi_view_limit: u8,
}

impl MediaTransportCapability {
    pub fn from_https_http2_verified(https_http2_verified: bool) -> Self {
        if https_http2_verified {
            Self {
                scheme: "https".to_string(),
                http_version: "h2".to_string(),
                multi_view_limit: 16,
            }
        } else {
            Self {
                scheme: "http".to_string(),
                http_version: "http/1.1".to_string(),
                multi_view_limit: 6,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MediaTransportCapability;

    #[test]
    fn media_transport_requires_verified_https_http2_for_sixteen_views() {
        let http = MediaTransportCapability::from_https_http2_verified(false);
        assert_eq!(http.scheme, "http");
        assert_eq!(http.http_version, "http/1.1");
        assert_eq!(http.multi_view_limit, 6);

        let https = MediaTransportCapability::from_https_http2_verified(true);
        assert_eq!(https.scheme, "https");
        assert_eq!(https.http_version, "h2");
        assert_eq!(https.multi_view_limit, 16);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum AiTaskSummaryState {
    Running,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct AiTaskSummary {
    pub task_id: String,
    pub model: String,
    pub stream_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub state: AiTaskSummaryState,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct RuntimeStatus {
    pub guard_available: bool,
    pub streams: usize,
    pub running_streams: usize,
    pub ai_tasks: usize,
    pub running_ai_tasks: usize,
    pub ptz_commands: u64,
}
