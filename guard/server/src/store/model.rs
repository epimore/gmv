use base_db::sqlx;

use crate::core::{
    ConnectionState, HealthState, LeaseState, NodeIdentity, RouteState, SchedulingState,
};

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
    pub capacity: u32,
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
    pub node_id: String,
    pub instance_id: String,
    pub idempotency_key: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFileInsert {
    pub id: i64,
    pub device_id: String,
    pub channel_id: String,
    pub biz_time: String,
    pub biz_id: String,
    pub file_type: i32,
    pub file_size: u64,
    pub file_name: String,
    pub file_format: Option<String>,
    pub dir_path: String,
    pub abs_path: Option<String>,
    pub note: Option<String>,
    pub is_del: i32,
    pub create_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmvRecordInsert {
    pub biz_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub user_id: Option<String>,
    pub st: String,
    pub et: String,
    pub speed: u32,
    pub ct: String,
    pub state: i32,
    pub lt: String,
    pub stream_app_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFileInsert {
    pub biz_id: String,
    pub file_size: u64,
    pub record_duration_sec: u64,
    pub file_format: Option<String>,
    pub dir_path: String,
    pub abs_path: Option<String>,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbDeviceRecord {
    pub device_id: String,
    pub session_node_id: String,
    pub alias: String,
    pub transport: String,
    pub device_type: String,
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub gb_version: String,
    pub local_addr: String,
    pub register_time: String,
    pub online_expire_time: String,
    pub status: String,
    pub camera_in_count: i64,
    pub camera_off_count: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbChannelRecord {
    pub device_id: String,
    pub channel_id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub owner: String,
    pub status: String,
    pub civil_code: String,
    pub address: String,
    pub parent_id: String,
    pub ip_address: String,
    pub port: i64,
    pub longitude: String,
    pub latitude: String,
    pub ptz_type: String,
    pub alias_name: String,
    pub pic_url: String,
    pub snapshot: i64,
    pub over_pic_id: String,
    pub ptz_enable: i64,
    pub talk_enable: i64,
    pub audio_enable: i64,
    pub record_enable: i64,
    pub playback_enable: i64,
    pub alarm_enable: i64,
    pub biz_enable: i64,
    pub sort_no: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbChannelImageRecord {
    pub image_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub image_url: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct GbDeviceRow {
    pub device_id: String,
    pub session_node_id: String,
    pub alias: String,
    pub transport: String,
    pub device_type: String,
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub gb_version: String,
    pub local_addr: String,
    pub register_time: String,
    pub online_expire_time: String,
    pub status: String,
    pub camera_in_count: i64,
    pub camera_off_count: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct GbChannelRow {
    pub device_id: String,
    pub channel_id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub owner: String,
    pub status: String,
    pub civil_code: String,
    pub address: String,
    pub parent_id: String,
    pub ip_address: String,
    pub port: i64,
    pub longitude: String,
    pub latitude: String,
    pub ptz_type: String,
    pub alias_name: String,
    pub pic_url: String,
    pub snapshot: i64,
    pub over_pic_id: String,
    pub ptz_enable: i64,
    pub talk_enable: i64,
    pub audio_enable: i64,
    pub record_enable: i64,
    pub playback_enable: i64,
    pub alarm_enable: i64,
    pub biz_enable: i64,
    pub sort_no: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct GbChannelImageRow {
    pub image_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub image_url: String,
    pub created_at_ms: i64,
}

pub(crate) fn gb_device_from_row(row: GbDeviceRow) -> GbDeviceRecord {
    GbDeviceRecord {
        device_id: row.device_id,
        session_node_id: row.session_node_id,
        alias: row.alias,
        transport: row.transport,
        device_type: row.device_type,
        manufacturer: row.manufacturer,
        model: row.model,
        firmware: row.firmware,
        gb_version: row.gb_version,
        local_addr: row.local_addr,
        register_time: row.register_time,
        online_expire_time: row.online_expire_time,
        status: row.status,
        camera_in_count: row.camera_in_count,
        camera_off_count: row.camera_off_count,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    }
}

pub(crate) fn gb_channel_from_row(row: GbChannelRow) -> GbChannelRecord {
    GbChannelRecord {
        device_id: row.device_id,
        channel_id: row.channel_id,
        name: row.name,
        manufacturer: row.manufacturer,
        model: row.model,
        owner: row.owner,
        status: row.status,
        civil_code: row.civil_code,
        address: row.address,
        parent_id: row.parent_id,
        ip_address: row.ip_address,
        port: row.port,
        longitude: row.longitude,
        latitude: row.latitude,
        ptz_type: row.ptz_type,
        alias_name: row.alias_name,
        pic_url: row.pic_url,
        snapshot: row.snapshot,
        over_pic_id: row.over_pic_id,
        ptz_enable: row.ptz_enable,
        talk_enable: row.talk_enable,
        audio_enable: row.audio_enable,
        record_enable: row.record_enable,
        playback_enable: row.playback_enable,
        alarm_enable: row.alarm_enable,
        biz_enable: row.biz_enable,
        sort_no: row.sort_no,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    }
}

pub(crate) fn gb_channel_image_from_row(row: GbChannelImageRow) -> GbChannelImageRecord {
    GbChannelImageRecord {
        image_id: row.image_id,
        device_id: row.device_id,
        channel_id: row.channel_id,
        image_url: row.image_url,
        created_at_ms: row.created_at_ms,
    }
}
