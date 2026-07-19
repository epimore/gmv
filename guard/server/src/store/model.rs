use crate::auth::Role;
use crate::core::{
    ConnectionState, HealthState, LeaseState, NodeIdentity, RouteState, SchedulingState,
};

use std::collections::HashMap;

pub const PLAYBACK_TOKEN_TTL_MS: i64 = 60_000;
pub const PLAYBACK_PRESENCE_TTL_MS: i64 = 181_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostMetricsRecord {
    pub cpu_usage_percent: f64,
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub disk_read_bytes_per_sec: u64,
    pub disk_write_bytes_per_sec: u64,
    pub network_receive_bytes_per_sec: u64,
    pub network_transmit_bytes_per_sec: u64,
    pub process_resident_memory_bytes: u64,
    pub process_threads: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeRecord {
    pub identity: NodeIdentity,
    pub connection: ConnectionState,
    pub health: HealthState,
    pub scheduling: SchedulingState,
    pub endpoints: Vec<EndpointRecord>,
    pub capabilities: Vec<String>,
    pub pending_leases: u32,
    pub host_metrics: HostMetricsRecord,
    pub business_metrics: std::collections::HashMap<String, String>,
    pub config: std::collections::HashMap<String, String>,
    pub zone: Option<String>,
    pub last_seen_at_ms: i64,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRecord {
    pub name: String,
    pub scheme: String,
    pub host: String,
    pub port: u32,
    pub mode: EndpointModeRecord,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointModeRecord {
    Single,
    Multi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRecord {
    pub lease_id: String,
    pub route_id: String,
    pub resource_id: String,
    pub stream_type: String,
    pub node_id: String,
    pub instance_id: String,
    pub idempotency_key: String,
    pub constraints: HashMap<String, String>,
    pub state: LeaseState,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRecord {
    pub route_id: String,
    pub resource_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub state: RouteState,
    pub desired_generation: u64,
    pub observed_generation: u64,
    pub observed_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackTicketRecord {
    pub token: String,
    pub stream_id: String,
    pub playback_id: String,
    pub playback_start_time_sec: u32,
    pub playback_end_time_sec: u32,
    pub output_id: String,
    pub subscription_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub username: String,
    pub ui_session_token: String,
    pub required_role: Role,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackPresenceRecord {
    pub playback_id: String,
    pub stream_id: String,
    pub subscription_id: String,
    pub username: String,
    pub ui_session_token: String,
    pub generation: u64,
    pub expires_at_ms: i64,
    pub control_in_flight: bool,
    pub closing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSessionOwnerRecord {
    pub stream_id: String,
    pub input_key: String,
    pub node_id: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: String,
    pub topic: String,
    pub priority: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxDestinationKind {
    Mqtt,
    Webhook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxState {
    Pending,
    Sending,
    Delivered,
    RetryWait,
    Dead,
}

impl OutboxState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Delivered | Self::Dead)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    pub outbox_id: String,
    pub event_id: String,
    pub destination_kind: OutboxDestinationKind,
    pub destination: String,
    pub payload: Vec<u8>,
    pub state: OutboxState,
    pub attempts: u32,
    pub next_attempt_at_ms: i64,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl OutboxDestinationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mqtt => "MQTT",
            Self::Webhook => "WEBHOOK",
        }
    }

    pub(crate) fn parse(value: &str) -> crate::core::GuardResult<Self> {
        match value {
            "MQTT" => Ok(Self::Mqtt),
            "WEBHOOK" => Ok(Self::Webhook),
            _ => Err(crate::core::GuardError::InvalidConfig(format!(
                "invalid outbox destination kind {value}"
            ))),
        }
    }
}

impl OutboxState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Sending => "SENDING",
            Self::Delivered => "DELIVERED",
            Self::RetryWait => "RETRY_WAIT",
            Self::Dead => "DEAD",
        }
    }

    pub(crate) fn parse(value: &str) -> crate::core::GuardResult<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "SENDING" => Ok(Self::Sending),
            "DELIVERED" => Ok(Self::Delivered),
            "RETRY_WAIT" => Ok(Self::RetryWait),
            "DEAD" => Ok(Self::Dead),
            _ => Err(crate::core::GuardError::InvalidConfig(format!(
                "invalid outbox state {value}"
            ))),
        }
    }
}

pub(crate) type OutboxRow = (
    String,
    String,
    String,
    String,
    Vec<u8>,
    String,
    i64,
    i64,
    Option<String>,
    i64,
    i64,
);

pub(crate) fn outbox_from_row(row: OutboxRow) -> crate::core::GuardResult<OutboxRecord> {
    Ok(OutboxRecord {
        outbox_id: row.0,
        event_id: row.1,
        destination_kind: OutboxDestinationKind::parse(&row.2)?,
        destination: row.3,
        payload: row.4,
        state: OutboxState::parse(&row.5)?,
        attempts: u32::try_from(row.6).map_err(|_| {
            crate::core::GuardError::InvalidConfig("outbox attempts overflow".to_string())
        })?,
        next_attempt_at_ms: row.7,
        last_error: row.8,
        created_at_ms: row.9,
        updated_at_ms: row.10,
    })
}
