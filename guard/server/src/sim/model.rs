#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum EndpointMode {
    Single,
    Multi,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct SimDevice {
    pub device_id: String,
    pub name: String,
    pub session_node_id: String,
    pub channels: Vec<String>,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum SimStreamState {
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct SimStream {
    pub stream_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub endpoint: String,
    pub state: SimStreamState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum SimAiTaskState {
    Running,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct SimAiTask {
    pub task_id: String,
    pub model: String,
    pub stream_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub state: SimAiTaskState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimFaults {
    pub fail_next_stream_start: bool,
    pub fail_next_ai_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct SimStatus {
    pub guard_available: bool,
    pub streams: usize,
    pub running_streams: usize,
    pub ai_tasks: usize,
    pub running_ai_tasks: usize,
    pub ptz_commands: u64,
}
