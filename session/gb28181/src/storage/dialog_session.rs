use std::fmt::{Display, Formatter};
use std::str::FromStr;

use crate::storage::db;
use base::chrono::NaiveDateTime;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde_json;
#[cfg(feature = "db-mysql")]
use base_db::sqlx::MySql;
#[cfg(feature = "db-sqlite")]
use base_db::sqlx::Sqlite;
use base_db::sqlx::{self, FromRow};

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};

const INSERT_COLUMNS: &str = "stream_id,parent_stream_id,device_id,channel_id,session_type,requested_stream_profile,effective_stream_profile,stream_profile_verification,\
signal_node_id,media_node_id,ssrc,registration_epoch_id,call_id,local_uri,remote_uri,local_tag,remote_tag,\
local_cseq,remote_cseq,contact_uri,route_set,local_sip_addr,remote_sip_addr,transport,\
state,established_at,terminated_at,terminal_reason,stop_reason,error_code,last_seen_at,expire_at,version,created_at,updated_at";
const SELECT_COLUMNS: &str = "stream_id,parent_stream_id,device_id,channel_id,session_type,requested_stream_profile,effective_stream_profile,stream_profile_verification,signal_node_id,\
media_node_id,ssrc,registration_epoch_id,call_id,local_uri,remote_uri,local_tag,remote_tag,local_cseq,remote_cseq,\
contact_uri,route_set,local_sip_addr,remote_sip_addr,transport,state,established_at,terminated_at,terminal_reason,stop_reason,error_code,last_seen_at,\
expire_at,version,created_at,updated_at";
const MAX_PAGE_SIZE: u32 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogSessionType {
    Live,
    Playback,
    Download,
    Broadcast,
}

impl Display for DialogSessionType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Live => "LIVE",
            Self::Playback => "PLAYBACK",
            Self::Download => "DOWNLOAD",
            Self::Broadcast => "BROADCAST",
        })
    }
}

impl FromStr for DialogSessionType {
    type Err = GlobalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "LIVE" => Ok(Self::Live),
            "PLAYBACK" => Ok(Self::Playback),
            "DOWNLOAD" => Ok(Self::Download),
            "BROADCAST" => Ok(Self::Broadcast),
            _ => Err(invalid_data("invalid dialog session type")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogTransport {
    Udp,
    Tcp,
    Tls,
}

impl Display for DialogTransport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Udp => "UDP",
            Self::Tcp => "TCP",
            Self::Tls => "TLS",
        })
    }
}

impl FromStr for DialogTransport {
    type Err = GlobalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "UDP" => Ok(Self::Udp),
            "TCP" => Ok(Self::Tcp),
            "TLS" => Ok(Self::Tls),
            _ => Err(invalid_data("invalid dialog transport")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogState {
    Inviting,
    Established,
    Terminating,
    Terminated,
    Orphan,
}

impl DialogState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Terminated | Self::Orphan)
    }

    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Inviting,
                Self::Established | Self::Terminating | Self::Terminated | Self::Orphan
            ) | (
                Self::Established,
                Self::Terminating | Self::Terminated | Self::Orphan
            ) | (Self::Terminating, Self::Terminated | Self::Orphan)
        )
    }
}

impl Display for DialogState {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Inviting => "INVITING",
            Self::Established => "ESTABLISHED",
            Self::Terminating => "TERMINATING",
            Self::Terminated => "TERMINATED",
            Self::Orphan => "ORPHAN",
        })
    }
}

impl FromStr for DialogState {
    type Err = GlobalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "INVITING" => Ok(Self::Inviting),
            "ESTABLISHED" => Ok(Self::Established),
            "TERMINATING" => Ok(Self::Terminating),
            "TERMINATED" => Ok(Self::Terminated),
            "ORPHAN" => Ok(Self::Orphan),
            _ => Err(invalid_data("invalid dialog state")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipDialogSession {
    pub stream_id: String,
    pub parent_stream_id: Option<String>,
    pub device_id: String,
    pub channel_id: String,
    pub session_type: DialogSessionType,
    pub requested_stream_profile: Option<String>,
    pub effective_stream_profile: Option<String>,
    pub stream_profile_verification: Option<String>,
    pub signal_node_id: String,
    pub media_node_id: String,
    pub ssrc: Option<String>,
    pub registration_epoch_id: Option<String>,
    pub call_id: String,
    pub local_uri: String,
    pub remote_uri: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub local_cseq: i64,
    pub remote_cseq: Option<i64>,
    pub contact_uri: Option<String>,
    pub route_set: Vec<String>,
    pub local_sip_addr: String,
    pub remote_sip_addr: String,
    pub transport: DialogTransport,
    pub state: DialogState,
    pub established_at: Option<NaiveDateTime>,
    pub terminated_at: Option<NaiveDateTime>,
    pub terminal_reason: Option<String>,
    pub stop_reason: Option<String>,
    pub error_code: Option<String>,
    pub last_seen_at: NaiveDateTime,
    pub expire_at: NaiveDateTime,
    pub version: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl SipDialogSession {
    pub fn validate(&self) -> GlobalResult<()> {
        validate_len(&self.stream_id, 64, "stream_id")?;
        validate_len(&self.device_id, 32, "device_id")?;
        validate_len(&self.channel_id, 32, "channel_id")?;
        validate_optional_len(
            self.requested_stream_profile.as_deref(),
            16,
            "requested_stream_profile",
        )?;
        validate_optional_len(
            self.effective_stream_profile.as_deref(),
            16,
            "effective_stream_profile",
        )?;
        validate_optional_len(
            self.stream_profile_verification.as_deref(),
            16,
            "stream_profile_verification",
        )?;
        if self
            .requested_stream_profile
            .as_deref()
            .is_some_and(|value| !matches!(value, "main" | "sub"))
            || self
                .effective_stream_profile
                .as_deref()
                .is_some_and(|value| !matches!(value, "main" | "sub"))
            || self
                .stream_profile_verification
                .as_deref()
                .is_some_and(|value| !matches!(value, "CONFIRMED" | "UNVERIFIED"))
        {
            return Err(invalid_data("invalid live stream profile metadata"));
        }
        validate_len(&self.signal_node_id, 64, "signal_node_id")?;
        validate_len(&self.media_node_id, 64, "media_node_id")?;
        validate_optional_len(self.ssrc.as_deref(), 16, "ssrc")?;
        validate_optional_len(
            self.registration_epoch_id.as_deref(),
            36,
            "registration_epoch_id",
        )?;
        validate_optional_len(self.terminal_reason.as_deref(), 64, "terminal_reason")?;
        validate_optional_stop_reason(self.stop_reason.as_deref())?;
        validate_optional_len(self.error_code.as_deref(), 64, "error_code")?;
        validate_len(&self.call_id, 128, "call_id")?;
        validate_sip_uri(&self.local_uri, 256, "local_uri")?;
        validate_sip_uri(&self.remote_uri, 256, "remote_uri")?;
        validate_len(&self.local_tag, 128, "local_tag")?;
        validate_optional_len(self.remote_tag.as_deref(), 128, "remote_tag")?;
        validate_optional_sip_uri(self.contact_uri.as_deref(), 256, "contact_uri")?;
        validate_addr(&self.local_sip_addr, "local_sip_addr")?;
        validate_addr(&self.remote_sip_addr, "remote_sip_addr")?;
        validate_route_set(&self.route_set)?;
        if self.local_cseq <= 0 || self.remote_cseq.is_some_and(|value| value <= 0) {
            return Err(invalid_data("dialog CSeq must be positive"));
        }
        if self.version < 0
            || self.updated_at < self.created_at
            || self.last_seen_at < self.created_at
            || self.expire_at <= self.last_seen_at
            || self
                .established_at
                .is_some_and(|value| value < self.created_at || value > self.updated_at)
        {
            return Err(invalid_data("invalid dialog version or timestamps"));
        }
        match self.state {
            DialogState::Inviting if self.established_at.is_some() => {
                Err(invalid_data("INVITING must not have established_at"))
            }
            DialogState::Established | DialogState::Terminating
                if self.remote_tag.is_none() || self.established_at.is_none() =>
            {
                Err(invalid_data(
                    "established dialog states require remote_tag and established_at",
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EstablishedDialogFields {
    pub remote_tag: String,
    pub local_cseq: i64,
    pub remote_cseq: Option<i64>,
    pub contact_uri: Option<String>,
    pub route_set: Vec<String>,
    pub local_sip_addr: String,
    pub remote_sip_addr: String,
    pub established_at: NaiveDateTime,
    pub last_seen_at: NaiveDateTime,
    pub expire_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl EstablishedDialogFields {
    fn validate(&self) -> GlobalResult<()> {
        validate_len(&self.remote_tag, 128, "remote_tag")?;
        validate_optional_sip_uri(self.contact_uri.as_deref(), 256, "contact_uri")?;
        validate_addr(&self.local_sip_addr, "local_sip_addr")?;
        validate_addr(&self.remote_sip_addr, "remote_sip_addr")?;
        validate_route_set(&self.route_set)?;
        if self.local_cseq <= 0
            || self.remote_cseq.is_some_and(|value| value <= 0)
            || self.last_seen_at < self.established_at
            || self.updated_at < self.established_at
            || self.expire_at <= self.last_seen_at
        {
            return Err(invalid_data("invalid established dialog fields"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, FromRow)]
struct SipDialogSessionRow {
    stream_id: String,
    parent_stream_id: Option<String>,
    device_id: String,
    channel_id: String,
    session_type: String,
    requested_stream_profile: Option<String>,
    effective_stream_profile: Option<String>,
    stream_profile_verification: Option<String>,
    signal_node_id: String,
    media_node_id: String,
    ssrc: Option<String>,
    registration_epoch_id: Option<String>,
    call_id: String,
    local_uri: String,
    remote_uri: String,
    local_tag: String,
    remote_tag: Option<String>,
    local_cseq: i64,
    remote_cseq: Option<i64>,
    contact_uri: Option<String>,
    route_set: Option<String>,
    local_sip_addr: String,
    remote_sip_addr: String,
    transport: String,
    state: String,
    established_at: Option<NaiveDateTime>,
    terminated_at: Option<NaiveDateTime>,
    terminal_reason: Option<String>,
    stop_reason: Option<String>,
    error_code: Option<String>,
    last_seen_at: NaiveDateTime,
    expire_at: NaiveDateTime,
    version: i64,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

impl TryFrom<SipDialogSessionRow> for SipDialogSession {
    type Error = GlobalError;

    fn try_from(row: SipDialogSessionRow) -> Result<Self, Self::Error> {
        let session = Self {
            stream_id: row.stream_id,
            parent_stream_id: row.parent_stream_id,
            device_id: row.device_id,
            channel_id: row.channel_id,
            session_type: row.session_type.parse()?,
            requested_stream_profile: row.requested_stream_profile,
            effective_stream_profile: row.effective_stream_profile,
            stream_profile_verification: row.stream_profile_verification,
            signal_node_id: row.signal_node_id,
            media_node_id: row.media_node_id,
            ssrc: row.ssrc,
            registration_epoch_id: row.registration_epoch_id,
            call_id: row.call_id,
            local_uri: row.local_uri,
            remote_uri: row.remote_uri,
            local_tag: row.local_tag,
            remote_tag: row.remote_tag,
            local_cseq: row.local_cseq,
            remote_cseq: row.remote_cseq,
            contact_uri: row.contact_uri,
            route_set: route_set_from_json(row.route_set.as_deref())?,
            local_sip_addr: row.local_sip_addr,
            remote_sip_addr: row.remote_sip_addr,
            transport: row.transport.parse()?,
            state: row.state.parse()?,
            established_at: row.established_at,
            terminated_at: row.terminated_at,
            terminal_reason: row.terminal_reason,
            stop_reason: row.stop_reason,
            error_code: row.error_code,
            last_seen_at: row.last_seen_at,
            expire_at: row.expire_at,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        };
        session.validate()?;
        Ok(session)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackPauseLease {
    pub playback_id: String,
    pub state: String,
    pub expire_at: Option<NaiveDateTime>,
    pub generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogMonitorFilter {
    pub stream_id: String,
    pub media_node_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub ssrc: String,
    pub state: String,
}

impl DialogMonitorFilter {
    fn matches(&self, session: &SipDialogSession) -> bool {
        (self.stream_id.is_empty() || session.stream_id == self.stream_id)
            && (self.media_node_id.is_empty() || session.media_node_id == self.media_node_id)
            && (self.device_id.is_empty() || session.device_id == self.device_id)
            && (self.channel_id.is_empty() || session.channel_id == self.channel_id)
            && (self.ssrc.is_empty() || session.ssrc.as_deref() == Some(self.ssrc.as_str()))
            && (self.state.is_empty() || session.state.to_string() == self.state)
    }

    fn validate(&self) -> GlobalResult<()> {
        validate_optional_len(
            (!self.stream_id.is_empty()).then_some(self.stream_id.as_str()),
            64,
            "stream_id",
        )?;
        validate_optional_len(
            (!self.media_node_id.is_empty()).then_some(self.media_node_id.as_str()),
            64,
            "media_node_id",
        )?;
        validate_optional_len(
            (!self.device_id.is_empty()).then_some(self.device_id.as_str()),
            32,
            "device_id",
        )?;
        validate_optional_len(
            (!self.channel_id.is_empty()).then_some(self.channel_id.as_str()),
            32,
            "channel_id",
        )?;
        validate_optional_len(
            (!self.ssrc.is_empty()).then_some(self.ssrc.as_str()),
            16,
            "ssrc",
        )?;
        Ok(())
    }
}

pub struct SipDialogSessionRepository;

impl SipDialogSessionRepository {
    pub async fn insert_inviting(session: &SipDialogSession) -> GlobalResult<()> {
        session.validate()?;
        if session.state != DialogState::Inviting
            || session.version != 0
            || session.remote_tag.is_some()
        {
            return Err(invalid_data(
                "insert_inviting requires INVITING version 0 without remote_tag",
            ));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if storage.contains_key(&session.stream_id) {
                return Err(invalid_data("duplicate dialog stream_id"));
            }
            storage.insert(session.stream_id.clone(), session.clone());
            return Ok(());
        }

        let route_set = route_set_to_json(&session.route_set)?;
        db::execute!(
            sqlx::AssertSqlSafe(format!(
                "INSERT INTO gb28181_sip_dialog_session ({INSERT_COLUMNS}) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
            )),
            &session.stream_id,
            &session.parent_stream_id,
            &session.device_id,
            &session.channel_id,
            session.session_type.to_string(),
            &session.requested_stream_profile,
            &session.effective_stream_profile,
            &session.stream_profile_verification,
            &session.signal_node_id,
            &session.media_node_id,
            &session.ssrc,
            &session.registration_epoch_id,
            &session.call_id,
            &session.local_uri,
            &session.remote_uri,
            &session.local_tag,
            &session.remote_tag,
            session.local_cseq,
            session.remote_cseq,
            &session.contact_uri,
            route_set,
            &session.local_sip_addr,
            &session.remote_sip_addr,
            session.transport.to_string(),
            session.state.to_string(),
            session.established_at,
            session.terminated_at,
            &session.terminal_reason,
            &session.stop_reason,
            &session.error_code,
            session.last_seen_at,
            session.expire_at,
            session.version,
            session.created_at,
            session.updated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(())
    }

    pub async fn update_stream_profile_verification(
        stream_id: &str,
        signal_node_id: &str,
        effective_stream_profile: &str,
        verification: &str,
        updated_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        validate_len(effective_stream_profile, 16, "effective_stream_profile")?;
        validate_len(verification, 16, "stream_profile_verification")?;

        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.signal_node_id != signal_node_id
                || session.session_type != DialogSessionType::Live
            {
                return Ok(false);
            }
            session.effective_stream_profile = Some(effective_stream_profile.to_string());
            session.stream_profile_verification = Some(verification.to_string());
            if updated_at > session.updated_at {
                session.updated_at = updated_at;
            }
            session.version += 1;
            return Ok(true);
        }

        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET effective_stream_profile=?,\
             stream_profile_verification=?,version=version+1,\
             updated_at=CASE WHEN updated_at>? THEN updated_at ELSE ? END \
             WHERE stream_id=? AND signal_node_id=? AND session_type='LIVE' \
             AND state IN ('INVITING','ESTABLISHED')",
            effective_stream_profile,
            verification,
            updated_at,
            updated_at,
            stream_id,
            signal_node_id,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn cas_mark_established(
        stream_id: &str,
        signal_node_id: &str,
        expected_registration_epoch_id: Option<&str>,
        expected_version: i64,
        fields: &EstablishedDialogFields,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        fields.validate()?;
        if expected_version < 0 {
            return Err(invalid_data("expected_version must not be negative"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.state != DialogState::Inviting
                || session.signal_node_id != signal_node_id
                || session.registration_epoch_id.as_deref() != expected_registration_epoch_id
                || fields.established_at < session.created_at
                || fields.updated_at < session.updated_at
            {
                return Ok(false);
            }
            session.remote_tag = Some(fields.remote_tag.clone());
            session.local_cseq = fields.local_cseq;
            session.remote_cseq = fields.remote_cseq;
            session.contact_uri = fields.contact_uri.clone();
            session.route_set = fields.route_set.clone();
            session.local_sip_addr = fields.local_sip_addr.clone();
            session.remote_sip_addr = fields.remote_sip_addr.clone();
            session.state = DialogState::Established;
            session.established_at = Some(fields.established_at);
            session.last_seen_at = fields.last_seen_at;
            session.expire_at = fields.expire_at;
            session.updated_at = fields.updated_at;
            session.version += 1;
            return Ok(true);
        }

        let route_set = route_set_to_json(&fields.route_set)?;
        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET remote_tag=?,local_cseq=?,remote_cseq=?,\
             contact_uri=?,route_set=?,local_sip_addr=?,remote_sip_addr=?,state='ESTABLISHED',\
             established_at=?,last_seen_at=?,expire_at=?,updated_at=?,version=version+1 \
             WHERE stream_id=? AND signal_node_id=? AND state='INVITING' AND version=? \
             AND ((registration_epoch_id IS NULL AND ? IS NULL) OR registration_epoch_id=?) \
             AND created_at<=? AND updated_at<=?",
            &fields.remote_tag,
            fields.local_cseq,
            fields.remote_cseq,
            &fields.contact_uri,
            route_set,
            &fields.local_sip_addr,
            &fields.remote_sip_addr,
            fields.established_at,
            fields.last_seen_at,
            fields.expire_at,
            fields.updated_at,
            stream_id,
            signal_node_id,
            expected_version,
            expected_registration_epoch_id,
            expected_registration_epoch_id,
            fields.established_at,
            fields.updated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn cas_begin_terminating(
        stream_id: &str,
        signal_node_id: &str,
        expected_version: i64,
        current_cseq: i64,
        next_cseq: i64,
        updated_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        if expected_version < 0
            || current_cseq <= 0
            || next_cseq != current_cseq + 1
            || next_cseq > i64::from(i32::MAX)
        {
            return Err(invalid_data("invalid terminating CSeq reservation"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.state != DialogState::Established
                || session.signal_node_id != signal_node_id
                || session.local_cseq != current_cseq
                || updated_at < session.updated_at
            {
                return Ok(false);
            }
            session.local_cseq = next_cseq;
            session.state = DialogState::Terminating;
            session.updated_at = updated_at;
            session.version += 1;
            return Ok(true);
        }

        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET local_cseq=?,state='TERMINATING',\
             updated_at=?,version=version+1 WHERE stream_id=? AND signal_node_id=? \
             AND state='ESTABLISHED' AND local_cseq=? AND version=? AND updated_at<=?",
            next_cseq,
            updated_at,
            stream_id,
            signal_node_id,
            current_cseq,
            expected_version,
            updated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn cas_transition(
        stream_id: &str,
        signal_node_id: &str,
        expected_version: i64,
        expected_state: DialogState,
        next_state: DialogState,
        updated_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        if expected_version < 0
            || expected_state.is_terminal()
            || next_state.is_terminal()
            || !expected_state.can_transition_to(next_state)
        {
            return Err(invalid_data("invalid dialog state transition"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.state != expected_state
                || session.signal_node_id != signal_node_id
                || updated_at < session.updated_at
            {
                return Ok(false);
            }
            session.state = next_state;
            session.updated_at = updated_at;
            session.version += 1;
            return Ok(true);
        }

        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET state=?,updated_at=?,version=version+1 \
             WHERE stream_id=? AND signal_node_id=? AND state=? AND version=? AND updated_at<=?",
            next_state.to_string(),
            updated_at,
            stream_id,
            signal_node_id,
            expected_state.to_string(),
            expected_version,
            updated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn cas_mark_terminal(
        stream_id: &str,
        signal_node_id: &str,
        expected_version: i64,
        expected_state: DialogState,
        terminal_state: DialogState,
        terminal_reason: &str,
        error_code: Option<&str>,
        terminated_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(terminal_reason, 64, "terminal_reason")?;
        validate_optional_len(error_code, 64, "error_code")?;
        if expected_version < 0
            || !terminal_state.is_terminal()
            || expected_state.is_terminal()
            || !expected_state.can_transition_to(terminal_state)
            || (terminal_state == DialogState::Terminated && error_code.is_some())
            || (terminal_state == DialogState::Orphan && error_code.is_none())
        {
            return Err(invalid_data("invalid terminal dialog transition"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.state != expected_state
                || session.signal_node_id != signal_node_id
                || terminated_at < session.updated_at
            {
                return Ok(false);
            }
            session.state = terminal_state;
            session.terminated_at = Some(terminated_at);
            session.terminal_reason = Some(terminal_reason.to_string());
            session.error_code = error_code.map(ToString::to_string);
            session.updated_at = terminated_at;
            session.version += 1;
            return Ok(true);
        }
        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET state=?,terminated_at=COALESCE(terminated_at,?),\
             terminal_reason=COALESCE(terminal_reason,?),error_code=COALESCE(error_code,?),\
             updated_at=?,version=version+1 WHERE stream_id=? AND signal_node_id=? AND state=? \
             AND version=? AND updated_at<=?",
            terminal_state.to_string(),
            terminated_at,
            terminal_reason,
            error_code,
            terminated_at,
            stream_id,
            signal_node_id,
            expected_state.to_string(),
            expected_version,
            terminated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn record_stop_reason(
        stream_id: &str,
        signal_node_id: &str,
        stop_reason: &str,
    ) -> GlobalResult<()> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        validate_stop_reason(stop_reason)?;
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(session) = storage.get_mut(stream_id)
                && session.signal_node_id == signal_node_id
                && !session.state.is_terminal()
                && session.stop_reason.is_none()
            {
                session.stop_reason = Some(stop_reason.to_string());
            }
            return Ok(());
        }
        db::execute!(
            "UPDATE gb28181_sip_dialog_session SET stop_reason=? \
             WHERE stream_id=? AND signal_node_id=? AND state IN ('INVITING','ESTABLISHED','TERMINATING') \
             AND stop_reason IS NULL",
            stop_reason,
            stream_id,
            signal_node_id,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(())
    }

    pub async fn initialize_playback_control(
        stream_id: &str,
        playback_id: &str,
        start_sec: u32,
        end_sec: u32,
    ) -> GlobalResult<()> {
        validate_len(playback_id, 64, "playback_id")?;
        if start_sec == 0 || start_sec >= end_sec {
            return Err(invalid_data("invalid playback range"));
        }
        #[cfg(test)]
        if use_test_storage() {
            return Ok(());
        }
        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET playback_id=?,playback_start_sec=?,             playback_end_sec=?,playback_generation=0,mansrtsp_cseq=local_cseq,             acknowledged_position_sec=?,desired_rate_milli=1000,acknowledged_rate_milli=1000,             playback_state='PLAYING' WHERE stream_id=? AND session_type='PLAYBACK'",
            playback_id,
            i64::from(start_sec),
            i64::from(end_sec),
            i64::from(start_sec),
            stream_id,
        )
        .hand_log(|message| error!("{message}"))?;
        if rows != 1 {
            return Err(invalid_data("playback dialog was not initialized"));
        }
        Ok(())
    }

    pub async fn cas_ack_playback_control(
        stream_id: &str,
        playback_id: &str,
        expected_generation: u64,
        position_sec: Option<u32>,
        rate_milli: Option<i64>,
        playback_state: Option<&str>,
        pause_expire_at: Option<NaiveDateTime>,
        operation_id: &str,
    ) -> GlobalResult<bool> {
        validate_len(playback_id, 64, "playback_id")?;
        validate_len(operation_id, 128, "operation_id")?;
        if playback_state.is_some_and(|state| !matches!(state, "PLAYING" | "PAUSED")) {
            return Err(invalid_data("invalid playback state"));
        }
        let expected_generation = i64::try_from(expected_generation)
            .map_err(|_| invalid_data("playback generation overflow"))?;
        #[cfg(test)]
        if use_test_storage() {
            return Ok(true);
        }
        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET              playback_generation=playback_generation+1,             acknowledged_position_sec=COALESCE(?,acknowledged_position_sec),             desired_rate_milli=COALESCE(?,desired_rate_milli),             acknowledged_rate_milli=COALESCE(?,acknowledged_rate_milli),             playback_state=COALESCE(?,playback_state),             pause_expire_at=CASE WHEN ?='PAUSED' THEN ? WHEN ?='PLAYING' THEN NULL ELSE pause_expire_at END,             mansrtsp_cseq=local_cseq,last_control_operation_id=?,version=version+1              WHERE stream_id=? AND playback_id=? AND session_type='PLAYBACK'              AND playback_generation=?",
            position_sec.map(i64::from),
            rate_milli,
            rate_milli,
            playback_state,
            playback_state,
            pause_expire_at,
            playback_state,
            operation_id,
            stream_id,
            playback_id,
            expected_generation,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn cas_reserve_local_cseq(
        stream_id: &str,
        signal_node_id: &str,
        expected_version: i64,
        current_cseq: i64,
        next_cseq: i64,
        updated_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        if expected_version < 0
            || current_cseq <= 0
            || next_cseq != current_cseq + 1
            || next_cseq > i64::from(i32::MAX)
        {
            return Err(invalid_data("invalid local CSeq reservation"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.signal_node_id != signal_node_id
                || session.local_cseq != current_cseq
                || updated_at < session.updated_at
                || !matches!(
                    session.state,
                    DialogState::Established | DialogState::Terminating
                )
            {
                return Ok(false);
            }
            session.local_cseq = next_cseq;
            session.updated_at = updated_at;
            session.version += 1;
            return Ok(true);
        }

        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET local_cseq=?,updated_at=?,version=version+1 \
             WHERE stream_id=? AND signal_node_id=? AND state IN ('ESTABLISHED','TERMINATING') \
             AND local_cseq=? AND version=? AND updated_at<=?",
            next_cseq,
            updated_at,
            stream_id,
            signal_node_id,
            current_cseq,
            expected_version,
            updated_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn cas_touch(
        stream_id: &str,
        signal_node_id: &str,
        expected_version: i64,
        last_seen_at: NaiveDateTime,
        expire_at: NaiveDateTime,
    ) -> GlobalResult<bool> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(signal_node_id, 64, "signal_node_id")?;
        if expected_version < 0 || expire_at <= last_seen_at {
            return Err(invalid_data("invalid dialog activity timestamps"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = storage.get_mut(stream_id) else {
                return Ok(false);
            };
            if session.version != expected_version
                || session.signal_node_id != signal_node_id
                || last_seen_at < session.last_seen_at
                || !matches!(
                    session.state,
                    DialogState::Established | DialogState::Terminating
                )
            {
                return Ok(false);
            }
            session.last_seen_at = last_seen_at;
            session.expire_at = expire_at;
            session.updated_at = last_seen_at.max(session.updated_at);
            session.version += 1;
            return Ok(true);
        }

        let rows = db::execute!(
            "UPDATE gb28181_sip_dialog_session SET last_seen_at=?,expire_at=?,updated_at=?,\
             version=version+1 WHERE stream_id=? AND signal_node_id=? \
             AND state IN ('ESTABLISHED','TERMINATING') AND version=? \
             AND last_seen_at<=? AND updated_at<=?",
            last_seen_at,
            expire_at,
            last_seen_at,
            stream_id,
            signal_node_id,
            expected_version,
            last_seen_at,
            last_seen_at,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(rows == 1)
    }

    pub async fn find_by_stream_id(stream_id: &str) -> GlobalResult<Option<SipDialogSession>> {
        validate_len(stream_id, 64, "stream_id")?;
        #[cfg(test)]
        if use_test_storage() {
            return Ok(test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(stream_id)
                .cloned());
        }
        let row = db::fetch_optional_as!(
            SipDialogSessionRow,
            sqlx::AssertSqlSafe(format!(
                "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE stream_id=?"
            )),
            stream_id,
        )
        .hand_log(|message| error!("{message}"))?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn find_playback_range(
        stream_id: &str,
        playback_id: &str,
    ) -> GlobalResult<Option<(u32, u32)>> {
        validate_len(stream_id, 64, "stream_id")?;
        validate_len(playback_id, 64, "playback_id")?;
        #[cfg(test)]
        if use_test_storage() {
            return Ok(None);
        }
        let row: Option<(Option<i64>, Option<i64>)> = db::fetch_optional_as!(
            (Option<i64>, Option<i64>),
            "SELECT playback_start_sec,playback_end_sec FROM gb28181_sip_dialog_session WHERE stream_id=? AND playback_id=? AND session_type='PLAYBACK'",
            stream_id,
            playback_id,
        )
        .hand_log(|message| error!("{message}"))?;
        let Some((Some(start_sec), Some(end_sec))) = row else {
            return Ok(None);
        };
        let start_sec =
            u32::try_from(start_sec).map_err(|_| invalid_data("invalid playback start time"))?;
        let end_sec =
            u32::try_from(end_sec).map_err(|_| invalid_data("invalid playback end time"))?;
        if start_sec == 0 || start_sec >= end_sec {
            return Err(invalid_data("invalid playback range"));
        }
        Ok(Some((start_sec, end_sec)))
    }

    pub async fn find_playback_state(stream_id: &str) -> GlobalResult<Option<String>> {
        validate_len(stream_id, 64, "stream_id")?;
        #[cfg(test)]
        if use_test_storage() {
            return Ok(None);
        }
        let row: Option<(Option<String>,)> = db::fetch_optional_as!(
            (Option<String>,),
            "SELECT playback_state FROM gb28181_sip_dialog_session WHERE stream_id=? AND session_type='PLAYBACK' AND state='ESTABLISHED'",
            stream_id,
        )
        .hand_log(|message| error!("{message}"))?;
        Ok(row.and_then(|(state,)| state))
    }

    pub async fn find_playback_pause_lease(
        stream_id: &str,
    ) -> GlobalResult<Option<PlaybackPauseLease>> {
        validate_len(stream_id, 64, "stream_id")?;
        #[cfg(test)]
        if use_test_storage() {
            return Ok(None);
        }
        let row: Option<(Option<String>, Option<String>, Option<NaiveDateTime>, Option<i64>)> =
            db::fetch_optional_as!(
                (Option<String>, Option<String>, Option<NaiveDateTime>, Option<i64>),
                "SELECT playback_id,playback_state,pause_expire_at,playback_generation FROM gb28181_sip_dialog_session WHERE stream_id=? AND session_type='PLAYBACK' AND state='ESTABLISHED'",
                stream_id,
            )
            .hand_log(|message| error!("{message}"))?;
        let Some((Some(playback_id), Some(state), expire_at, generation)) = row else {
            return Ok(None);
        };
        let generation = u64::try_from(generation.unwrap_or_default())
            .map_err(|_| invalid_data("invalid playback generation"))?;
        Ok(Some(PlaybackPauseLease {
            playback_id,
            state,
            expire_at,
            generation,
        }))
    }

    pub async fn find_by_call_id(call_id: &str) -> GlobalResult<Vec<SipDialogSession>> {
        validate_len(call_id, 128, "call_id")?;
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| session.call_id == call_id)
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
            return Ok(sessions);
        }
        let rows = db::fetch_all_as!(
            SipDialogSessionRow,
            sqlx::AssertSqlSafe(format!(
                "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session \
             WHERE call_id=? ORDER BY stream_id"
            )),
            call_id,
        )
        .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn find_active_by_device_epoch(
        device_id: &str,
        registration_epoch_id: Option<&str>,
    ) -> GlobalResult<Vec<SipDialogSession>> {
        validate_len(device_id, 32, "device_id")?;
        validate_optional_len(registration_epoch_id, 36, "registration_epoch_id")?;
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.device_id == device_id
                        && session.registration_epoch_id.as_deref() == registration_epoch_id
                        && matches!(
                            session.state,
                            DialogState::Inviting
                                | DialogState::Established
                                | DialogState::Terminating
                        )
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
            return Ok(sessions);
        }
        let rows = db::fetch_all_as!(
            SipDialogSessionRow,
            sqlx::AssertSqlSafe(format!(
                "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session \
                 WHERE device_id=? AND ((registration_epoch_id IS NULL AND ? IS NULL) OR registration_epoch_id=?) \
                 AND state IN ('INVITING','ESTABLISHED','TERMINATING') ORDER BY stream_id"
            )),
            device_id,
            registration_epoch_id,
            registration_epoch_id,
        )
        .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn find_active_by_media_ssrc_before(
        signal_node_id: &str,
        media_node_id: &str,
        ssrc: &str,
        first_seen_at: NaiveDateTime,
        now: NaiveDateTime,
    ) -> GlobalResult<Vec<SipDialogSession>> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        validate_len(media_node_id, 64, "media_node_id")?;
        validate_optional_len(Some(ssrc), 16, "ssrc")?;
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && session.media_node_id == media_node_id
                        && session.ssrc.as_deref() == Some(ssrc)
                        && session.session_type != DialogSessionType::Broadcast
                        && matches!(
                            session.state,
                            DialogState::Established | DialogState::Terminating
                        )
                        && session.created_at <= first_seen_at
                        && session.expire_at > now
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| right.created_at.cmp(&left.created_at));
            sessions.truncate(2);
            return Ok(sessions);
        }

        let rows = db::fetch_all_as!(
            SipDialogSessionRow,
            sqlx::AssertSqlSafe(format!(
                "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session              WHERE signal_node_id=? AND media_node_id=? AND ssrc=?              AND session_type IN ('LIVE','PLAYBACK','DOWNLOAD')              AND state IN ('ESTABLISHED','TERMINATING')              AND created_at<=? AND expire_at>?              ORDER BY created_at DESC LIMIT 2"
            )),
            signal_node_id,
            media_node_id,
            ssrc,
            first_seen_at,
            now,
        )
        .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn page_owned_by_states(
        signal_node_id: &str,
        states: &[DialogState],
        after_stream_id: Option<&str>,
        limit: u32,
    ) -> GlobalResult<Vec<SipDialogSession>> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        validate_optional_len(after_stream_id, 64, "after_stream_id")?;
        if states.is_empty() || limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(invalid_data("invalid owner page states or limit"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && states.contains(&session.state)
                        && after_stream_id.is_none_or(|cursor| session.stream_id.as_str() > cursor)
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
            sessions.truncate(limit as usize);
            return Ok(sessions);
        }

        let rows = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => {
                let mut builder = sqlx::QueryBuilder::<MySql>::new(format!(
                    "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE signal_node_id=",
                ));
                builder.push_bind(signal_node_id).push(" AND STATE IN (");
                let mut separated = builder.separated(",");
                for state in states {
                    separated.push_bind(state.to_string());
                }
                separated.push_unseparated(")");
                if let Some(cursor) = after_stream_id {
                    builder.push(" AND stream_id>").push_bind(cursor);
                }
                builder.push(" ORDER BY stream_id LIMIT ").push_bind(limit);
                builder
                    .build_query_as::<SipDialogSessionRow>()
                    .fetch_all(db::mysql_pool())
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => {
                let mut builder = sqlx::QueryBuilder::<Sqlite>::new(format!(
                    "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE signal_node_id=",
                ));
                builder.push_bind(signal_node_id).push(" AND STATE IN (");
                let mut separated = builder.separated(",");
                for state in states {
                    separated.push_bind(state.to_string());
                }
                separated.push_unseparated(")");
                if let Some(cursor) = after_stream_id {
                    builder.push(" AND stream_id>").push_bind(cursor);
                }
                builder.push(" ORDER BY stream_id LIMIT ").push_bind(limit);
                builder
                    .build_query_as::<SipDialogSessionRow>()
                    .fetch_all(db::sqlite_pool())
                    .await
            }
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn page_active_for_monitor(
        signal_node_id: &str,
        after_stream_id: Option<&str>,
        limit: u32,
        filter: &DialogMonitorFilter,
    ) -> GlobalResult<Vec<SipDialogSession>> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        validate_optional_len(after_stream_id, 64, "after_stream_id")?;
        filter.validate()?;
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(invalid_data("invalid monitor page limit"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && matches!(
                            session.state,
                            DialogState::Inviting
                                | DialogState::Established
                                | DialogState::Terminating
                        )
                        && after_stream_id.is_none_or(|cursor| session.stream_id.as_str() > cursor)
                        && filter.matches(session)
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
            sessions.truncate(limit as usize);
            return Ok(sessions);
        }
        macro_rules! build_query {
            ($backend:ty, $pool:expr) => {{
                let mut builder = sqlx::QueryBuilder::<$backend>::new(format!(
                    "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE signal_node_id=",
                ));
                builder
                    .push_bind(signal_node_id)
                    .push(" AND state IN ('INVITING','ESTABLISHED','TERMINATING')");
                if let Some(cursor) = after_stream_id {
                    builder.push(" AND stream_id>").push_bind(cursor);
                }
                if !filter.stream_id.is_empty() {
                    builder.push(" AND stream_id=").push_bind(&filter.stream_id);
                }
                if !filter.media_node_id.is_empty() {
                    builder
                        .push(" AND media_node_id=")
                        .push_bind(&filter.media_node_id);
                }
                if !filter.device_id.is_empty() {
                    builder.push(" AND device_id=").push_bind(&filter.device_id);
                }
                if !filter.channel_id.is_empty() {
                    builder
                        .push(" AND channel_id=")
                        .push_bind(&filter.channel_id);
                }
                if !filter.ssrc.is_empty() {
                    builder.push(" AND ssrc=").push_bind(&filter.ssrc);
                }
                builder.push(" ORDER BY stream_id LIMIT ").push_bind(limit);
                builder
                    .build_query_as::<SipDialogSessionRow>()
                    .fetch_all($pool)
                    .await
            }};
        }
        let rows = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => build_query!(MySql, db::mysql_pool()),
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => build_query!(Sqlite, db::sqlite_pool()),
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn page_active_dialogs_for_monitor(
        signal_node_id: &str,
        page: u32,
        page_size: u32,
        filter: &DialogMonitorFilter,
    ) -> GlobalResult<(Vec<SipDialogSession>, u64)> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        filter.validate()?;
        if page == 0 || page_size == 0 || page_size > 100 {
            return Err(invalid_data("invalid active dialog page"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && !session.state.is_terminal()
                        && filter.matches(session)
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| left.stream_id.cmp(&right.stream_id));
            let total = sessions.len() as u64;
            let offset = (u64::from(page - 1) * u64::from(page_size)) as usize;
            return Ok((
                sessions
                    .into_iter()
                    .skip(offset)
                    .take(page_size as usize)
                    .collect(),
                total,
            ));
        }
        macro_rules! append_active_where {
            ($builder:ident) => {{
                $builder
                    .push_bind(signal_node_id)
                    .push(" AND state IN ('INVITING','ESTABLISHED','TERMINATING')");
                if !filter.state.is_empty() {
                    $builder.push(" AND state=").push_bind(&filter.state);
                }
                if !filter.stream_id.is_empty() {
                    $builder
                        .push(" AND stream_id=")
                        .push_bind(&filter.stream_id);
                }
                if !filter.media_node_id.is_empty() {
                    $builder
                        .push(" AND media_node_id=")
                        .push_bind(&filter.media_node_id);
                }
                if !filter.device_id.is_empty() {
                    $builder
                        .push(" AND device_id=")
                        .push_bind(&filter.device_id);
                }
                if !filter.channel_id.is_empty() {
                    $builder
                        .push(" AND channel_id=")
                        .push_bind(&filter.channel_id);
                }
                if !filter.ssrc.is_empty() {
                    $builder.push(" AND ssrc=").push_bind(&filter.ssrc);
                }
            }};
        }
        macro_rules! run_active {
            ($backend:ty, $pool:expr) => {{
                async {
                    let mut transaction = $pool.begin().await?;
                    let mut count_builder = sqlx::QueryBuilder::<$backend>::new(
                        "SELECT COUNT(*) FROM gb28181_sip_dialog_session WHERE signal_node_id=",
                    );
                    append_active_where!(count_builder);
                    let (total,): (i64,) = count_builder
                        .build_query_as()
                        .fetch_one(&mut *transaction)
                        .await?;
                    let mut item_builder = sqlx::QueryBuilder::<$backend>::new(format!(
                        "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE signal_node_id="
                    ));
                    append_active_where!(item_builder);
                    item_builder
                        .push(" ORDER BY stream_id ASC LIMIT ")
                        .push_bind(page_size)
                        .push(" OFFSET ")
                        .push_bind(i64::from(page - 1) * i64::from(page_size));
                    let rows = item_builder
                        .build_query_as::<SipDialogSessionRow>()
                        .fetch_all(&mut *transaction)
                        .await?;
                    transaction.commit().await?;
                    Ok::<_, sqlx::Error>((rows, total))
                }
                .await
            }};
        }
        let (rows, total) = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => run_active!(MySql, db::mysql_pool()),
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => run_active!(Sqlite, db::sqlite_pool()),
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|message| error!("{message}"))?;
        let rows = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<GlobalResult<Vec<_>>>()?;
        Ok((rows, u64::try_from(total).unwrap_or_default()))
    }

    pub async fn page_history_for_monitor(
        signal_node_id: &str,
        page: u32,
        page_size: u32,
        filter: &DialogMonitorFilter,
    ) -> GlobalResult<(Vec<SipDialogSession>, u64)> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        filter.validate()?;
        if page == 0 || page_size == 0 || page_size > 100 {
            return Err(invalid_data("invalid history page"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut sessions = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && session.state.is_terminal()
                        && filter.matches(session)
                })
                .cloned()
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| {
                let left_at = left.terminated_at.unwrap_or(left.updated_at);
                let right_at = right.terminated_at.unwrap_or(right.updated_at);
                right_at
                    .cmp(&left_at)
                    .then_with(|| right.stream_id.cmp(&left.stream_id))
            });
            let total = sessions.len() as u64;
            let offset = (page.saturating_sub(1) * page_size) as usize;
            return Ok((
                sessions
                    .into_iter()
                    .skip(offset)
                    .take(page_size as usize)
                    .collect(),
                total,
            ));
        }
        macro_rules! append_history_where {
            ($builder:ident) => {{
                $builder
                    .push_bind(signal_node_id)
                    .push(" AND state IN ('TERMINATED','ORPHAN')");
                if !filter.state.is_empty() {
                    $builder.push(" AND state=").push_bind(&filter.state);
                }
                if !filter.stream_id.is_empty() {
                    $builder
                        .push(" AND stream_id=")
                        .push_bind(&filter.stream_id);
                }
                if !filter.media_node_id.is_empty() {
                    $builder
                        .push(" AND media_node_id=")
                        .push_bind(&filter.media_node_id);
                }
                if !filter.device_id.is_empty() {
                    $builder
                        .push(" AND device_id=")
                        .push_bind(&filter.device_id);
                }
                if !filter.channel_id.is_empty() {
                    $builder
                        .push(" AND channel_id=")
                        .push_bind(&filter.channel_id);
                }
                if !filter.ssrc.is_empty() {
                    $builder.push(" AND ssrc=").push_bind(&filter.ssrc);
                }
            }};
        }
        macro_rules! run_history {
            ($backend:ty, $pool:expr) => {{
                async {
                    let mut transaction = $pool.begin().await?;
                    let mut count_builder = sqlx::QueryBuilder::<$backend>::new(
                        "SELECT COUNT(*) FROM gb28181_sip_dialog_session WHERE signal_node_id=",
                    );
                    append_history_where!(count_builder);
                    let (total,): (i64,) = count_builder
                        .build_query_as()
                        .fetch_one(&mut *transaction)
                        .await?;
                    let mut item_builder = sqlx::QueryBuilder::<$backend>::new(format!(
                        "SELECT {SELECT_COLUMNS} FROM gb28181_sip_dialog_session WHERE signal_node_id="
                    ));
                    append_history_where!(item_builder);
                    item_builder
                        .push(" ORDER BY COALESCE(terminated_at,updated_at) DESC,stream_id DESC LIMIT ")
                        .push_bind(page_size)
                        .push(" OFFSET ")
                        .push_bind((page - 1) * page_size);
                    let rows = item_builder
                        .build_query_as::<SipDialogSessionRow>()
                        .fetch_all(&mut *transaction)
                        .await?;
                    transaction.commit().await?;
                    Ok::<_, sqlx::Error>((rows, total))
                }
                .await
            }};
        }
        let (rows, total) = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => run_history!(MySql, db::mysql_pool()),
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => run_history!(Sqlite, db::sqlite_pool()),
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|message| error!("{message}"))?;
        let rows = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<GlobalResult<Vec<_>>>()?;
        Ok((rows, u64::try_from(total).unwrap_or_default()))
    }

    pub async fn delete_terminal_before(
        signal_node_id: &str,
        cutoff: NaiveDateTime,
        limit: u32,
    ) -> GlobalResult<u64> {
        validate_len(signal_node_id, 64, "signal_node_id")?;
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(invalid_data("invalid retention batch limit"));
        }
        #[cfg(test)]
        if use_test_storage() {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let ids = storage
                .values()
                .filter(|session| {
                    session.signal_node_id == signal_node_id
                        && session.state.is_terminal()
                        && session.terminated_at.unwrap_or(session.updated_at) < cutoff
                })
                .take(limit as usize)
                .map(|session| session.stream_id.clone())
                .collect::<Vec<_>>();
            let deleted = ids.len() as u64;
            for id in ids {
                storage.remove(&id);
            }
            return Ok(deleted);
        }
        let rows = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => db::execute!(
                "DELETE FROM gb28181_sip_dialog_session WHERE signal_node_id=? AND state IN ('TERMINATED','ORPHAN') AND COALESCE(terminated_at,updated_at)<? LIMIT ?",
                signal_node_id, cutoff, limit
            ),
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => db::execute!(
                "DELETE FROM gb28181_sip_dialog_session WHERE stream_id IN (SELECT stream_id FROM gb28181_sip_dialog_session WHERE signal_node_id=? AND state IN ('TERMINATED','ORPHAN') AND COALESCE(terminated_at,updated_at)<? LIMIT ?)",
                signal_node_id, cutoff, limit
            ),
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|message| error!("{message}"))?;
        Ok(rows)
    }
}

fn route_set_to_json(route_set: &[String]) -> GlobalResult<Option<String>> {
    validate_route_set(route_set)?;
    if route_set.is_empty() {
        return Ok(None);
    }
    let json = serde_json::to_string(route_set)
        .map_err(|_| invalid_data("failed to serialize dialog route set"))?;
    if json.len() > u16::MAX as usize {
        return Err(invalid_data("dialog route set exceeds TEXT capacity"));
    }
    Ok(Some(json))
}

fn route_set_from_json(value: Option<&str>) -> GlobalResult<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let route_set = serde_json::from_str::<Vec<String>>(value)
        .map_err(|_| invalid_data("invalid dialog route set JSON"))?;
    if value.len() > u16::MAX as usize {
        return Err(invalid_data("dialog route set exceeds TEXT capacity"));
    }
    validate_route_set(&route_set)?;
    Ok(route_set)
}

fn validate_route_set(route_set: &[String]) -> GlobalResult<()> {
    for route in route_set {
        validate_sip_uri(route, 1_024, "route")?;
    }
    Ok(())
}

fn validate_sip_uri(value: &str, max_len: usize, field: &str) -> GlobalResult<()> {
    validate_len(value, max_len, field)?;
    let uri = value.trim().trim_start_matches('<').trim_end_matches('>');
    if !uri.starts_with("sip:") && !uri.starts_with("sips:") {
        return Err(invalid_data("invalid SIP URI"));
    }
    Ok(())
}

fn validate_optional_sip_uri(value: Option<&str>, max_len: usize, field: &str) -> GlobalResult<()> {
    if let Some(value) = value {
        validate_sip_uri(value, max_len, field)?;
    }
    Ok(())
}

fn validate_addr(value: &str, field: &str) -> GlobalResult<()> {
    validate_len(value, 64, field)?;
    value
        .parse::<std::net::SocketAddr>()
        .map(|_| ())
        .map_err(|_| invalid_data("invalid SIP socket address"))
}

fn validate_len(value: &str, max_len: usize, field: &str) -> GlobalResult<()> {
    if value.is_empty()
        || value.len() > max_len
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
    {
        return Err(invalid_data(&format!("invalid {field}")));
    }
    Ok(())
}

fn validate_optional_len(value: Option<&str>, max_len: usize, field: &str) -> GlobalResult<()> {
    if let Some(value) = value {
        validate_len(value, max_len, field)?;
    }
    Ok(())
}

fn validate_stop_reason(value: &str) -> GlobalResult<()> {
    if value.is_empty() || value.chars().count() > 255 || value.contains('\0') {
        return Err(invalid_data("invalid stop_reason"));
    }
    Ok(())
}

fn validate_optional_stop_reason(value: Option<&str>) -> GlobalResult<()> {
    if let Some(value) = value {
        validate_stop_reason(value)?;
    }
    Ok(())
}

fn invalid_data(message: &str) -> GlobalError {
    GlobalError::new_sys_error(message, |log_message| error!("{log_message}"))
}

#[cfg(test)]
static TEST_STORAGE_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static TEST_STORAGE: OnceLock<Mutex<HashMap<String, SipDialogSession>>> = OnceLock::new();
#[cfg(test)]
static TEST_STORAGE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
fn test_storage() -> &'static Mutex<HashMap<String, SipDialogSession>> {
    TEST_STORAGE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn use_test_storage() -> bool {
    TEST_STORAGE_ENABLED.load(Ordering::Acquire)
}

#[cfg(test)]
pub(crate) struct TestStorageGuard {
    _lock: MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestStorageGuard {
    fn drop(&mut self) {
        TEST_STORAGE_ENABLED.store(false, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn enable_dialog_test_storage() -> TestStorageGuard {
    let lock = TEST_STORAGE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_storage()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    TEST_STORAGE_ENABLED.store(true, Ordering::Release);
    TestStorageGuard { _lock: lock }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(offset_millis: i64) -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-06-18 00:00:00.000", "%Y-%m-%d %H:%M:%S%.3f")
            .expect("parse test datetime")
            + base::chrono::Duration::milliseconds(offset_millis)
    }

    fn inviting(stream_id: &str) -> SipDialogSession {
        SipDialogSession {
            stream_id: stream_id.into(),
            parent_stream_id: None,
            device_id: "34020000001320000001".into(),
            channel_id: "34020000001320000101".into(),
            session_type: DialogSessionType::Playback,
            requested_stream_profile: None,
            effective_stream_profile: None,
            stream_profile_verification: None,
            signal_node_id: "session-1".into(),
            media_node_id: "media-1".into(),
            ssrc: Some("1100000001".into()),
            registration_epoch_id: None,
            call_id: format!("call-{stream_id}"),
            local_uri: "sip:platform@127.0.0.1:5060".into(),
            remote_uri: "sip:device@127.0.0.1:15060".into(),
            local_tag: format!("tag-{stream_id}"),
            remote_tag: None,
            local_cseq: 10,
            remote_cseq: None,
            contact_uri: None,
            route_set: Vec::new(),
            local_sip_addr: "127.0.0.1:5060".into(),
            remote_sip_addr: "127.0.0.1:15060".into(),
            transport: DialogTransport::Udp,
            state: DialogState::Inviting,
            established_at: None,
            terminated_at: None,
            terminal_reason: None,
            stop_reason: None,
            error_code: None,
            last_seen_at: at(1_000),
            expire_at: at(28_801_000),
            version: 0,
            created_at: at(1_000),
            updated_at: at(1_000),
        }
    }

    #[test]
    fn stream_profile_update_keeps_updated_at_monotonic() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            let mut session = inviting("profile-time-stream");
            session.session_type = DialogSessionType::Live;
            session.requested_stream_profile = Some("main".to_string());
            session.effective_stream_profile = Some("main".to_string());
            session.stream_profile_verification = Some("UNVERIFIED".to_string());
            SipDialogSessionRepository::insert_inviting(&session)
                .await
                .expect("insert live dialog");

            assert!(
                SipDialogSessionRepository::update_stream_profile_verification(
                    &session.stream_id,
                    &session.signal_node_id,
                    "main",
                    "CONFIRMED",
                    at(900),
                )
                .await
                .expect("update profile with an older timestamp")
            );
            let unchanged = SipDialogSessionRepository::find_by_stream_id(&session.stream_id)
                .await
                .expect("read live dialog")
                .expect("live dialog exists");
            assert_eq!(unchanged.updated_at, at(1_000));
            assert_eq!(unchanged.version, 1);

            assert!(
                SipDialogSessionRepository::update_stream_profile_verification(
                    &session.stream_id,
                    &session.signal_node_id,
                    "main",
                    "CONFIRMED",
                    at(1_100),
                )
                .await
                .expect("update profile with a newer timestamp")
            );
            let advanced = SipDialogSessionRepository::find_by_stream_id(&session.stream_id)
                .await
                .expect("read updated live dialog")
                .expect("updated live dialog exists");
            assert_eq!(advanced.updated_at, at(1_100));
            assert_eq!(advanced.version, 2);
        });
    }

    #[test]
    fn dialog_establishment_and_lookup_are_fenced_by_registration_epoch() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            let mut session = inviting("epoch-stream");
            session.registration_epoch_id = Some("epoch-a".into());
            SipDialogSessionRepository::insert_inviting(&session)
                .await
                .expect("insert epoch dialog");
            SipDialogSessionRepository::insert_inviting(&inviting("legacy-stream"))
                .await
                .expect("insert legacy dialog");
            let established = EstablishedDialogFields {
                remote_tag: "remote-tag".into(),
                local_cseq: 10,
                remote_cseq: Some(20),
                contact_uri: Some("sip:device@127.0.0.1:15060".into()),
                route_set: Vec::new(),
                local_sip_addr: "127.0.0.1:5060".into(),
                remote_sip_addr: "127.0.0.1:15060".into(),
                established_at: at(1_100),
                last_seen_at: at(1_100),
                expire_at: at(28_801_100),
                updated_at: at(1_100),
            };

            assert!(
                !SipDialogSessionRepository::cas_mark_established(
                    "epoch-stream",
                    "session-1",
                    Some("epoch-b"),
                    0,
                    &established,
                )
                .await
                .expect("reject stale epoch")
            );
            assert!(
                SipDialogSessionRepository::cas_mark_established(
                    "epoch-stream",
                    "session-1",
                    Some("epoch-a"),
                    0,
                    &established,
                )
                .await
                .expect("establish current epoch")
            );
            assert_eq!(
                SipDialogSessionRepository::find_active_by_device_epoch(
                    &session.device_id,
                    Some("epoch-a"),
                )
                .await
                .expect("find epoch dialogs")
                .len(),
                1
            );
            assert!(
                SipDialogSessionRepository::find_active_by_device_epoch(
                    &session.device_id,
                    Some("epoch-b"),
                )
                .await
                .expect("exclude other epoch")
                .is_empty()
            );
            assert_eq!(
                SipDialogSessionRepository::find_active_by_device_epoch(&session.device_id, None)
                    .await
                    .expect("find legacy epoch dialogs")
                    .len(),
                1
            );
        });
    }

    #[test]
    fn repository_enforces_insert_cas_paging_and_route_contracts() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            let first = inviting("stream-1");
            let second = inviting("stream-2");
            SipDialogSessionRepository::insert_inviting(&first)
                .await
                .expect("insert first INVITING");
            SipDialogSessionRepository::insert_inviting(&second)
                .await
                .expect("insert second INVITING");
            assert!(
                SipDialogSessionRepository::insert_inviting(&first)
                    .await
                    .is_err()
            );

            let established = EstablishedDialogFields {
                remote_tag: "remote-tag".into(),
                local_cseq: 10,
                remote_cseq: Some(20),
                contact_uri: Some("sip:device@127.0.0.1:15060".into()),
                route_set: vec![
                    "<sip:proxy-a@127.0.0.1:15061;lr>".into(),
                    "<sip:proxy-b@127.0.0.1:15062;lr>".into(),
                ],
                local_sip_addr: "127.0.0.1:5060".into(),
                remote_sip_addr: "127.0.0.1:15060".into(),
                established_at: at(1_100),
                last_seen_at: at(1_100),
                expire_at: at(28_801_100),
                updated_at: at(1_100),
            };
            assert!(
                SipDialogSessionRepository::cas_mark_established(
                    "stream-1",
                    "session-1",
                    None,
                    0,
                    &established,
                )
                .await
                .expect("establish first")
            );
            assert!(
                !SipDialogSessionRepository::cas_mark_established(
                    "stream-1",
                    "session-1",
                    None,
                    0,
                    &established,
                )
                .await
                .expect("CAS loser")
            );
            assert!(
                SipDialogSessionRepository::cas_begin_terminating(
                    "stream-1",
                    "session-1",
                    1,
                    10,
                    11,
                    at(1_200),
                )
                .await
                .expect("begin terminating")
            );
            assert!(
                SipDialogSessionRepository::cas_touch(
                    "stream-1",
                    "session-1",
                    2,
                    at(1_250),
                    at(28_801_250),
                )
                .await
                .expect("touch terminating dialog")
            );
            assert!(
                !SipDialogSessionRepository::cas_reserve_local_cseq(
                    "stream-1",
                    "other-session",
                    3,
                    11,
                    12,
                    at(1_250),
                )
                .await
                .expect("non-owner CSeq CAS loser")
            );
            assert!(
                SipDialogSessionRepository::cas_mark_terminal(
                    "stream-1",
                    "session-1",
                    3,
                    DialogState::Terminating,
                    DialogState::Terminated,
                    "session_close",
                    None,
                    at(1_400),
                )
                .await
                .expect("mark terminated")
            );

            let loaded = SipDialogSessionRepository::find_by_stream_id("stream-1")
                .await
                .expect("lookup stream")
                .expect("stored stream");
            assert_eq!(loaded.state, DialogState::Terminated);
            assert_eq!(loaded.local_cseq, 11);
            assert_eq!(loaded.updated_at.and_utc().timestamp_subsec_millis(), 400);
            assert_eq!(loaded.route_set, established.route_set);
            assert_eq!(
                SipDialogSessionRepository::find_by_call_id(&first.call_id)
                    .await
                    .expect("lookup call"),
                vec![loaded]
            );

            let page = SipDialogSessionRepository::page_owned_by_states(
                "session-1",
                &[DialogState::Inviting],
                Some("stream-1"),
                10,
            )
            .await
            .expect("page owner rows");
            assert_eq!(page, vec![second]);

            let json = route_set_to_json(&established.route_set)
                .expect("serialize routes")
                .expect("non-empty JSON");
            assert_eq!(
                route_set_from_json(Some(&json)).expect("parse routes"),
                established.route_set
            );
            assert!(
                route_set_from_json(Some("{}"))
                    .expect_err("reject non-array route JSON")
                    .to_string()
                    .contains("invalid dialog route set JSON")
            );
            assert!(
                SipDialogSessionRepository::page_owned_by_states("session-1", &[], None, 10,)
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn unknown_stream_lookup_requires_one_active_preexisting_dialog() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            let mut matching = inviting("unknown-match");
            matching.session_type = DialogSessionType::Live;
            matching.state = DialogState::Established;
            matching.created_at = at(1_000);
            matching.updated_at = at(1_000);

            let mut future = matching.clone();
            future.stream_id = "unknown-future".into();
            future.created_at = at(3_000);
            future.updated_at = at(3_000);

            let mut broadcast = matching.clone();
            broadcast.stream_id = "unknown-broadcast".into();
            broadcast.session_type = DialogSessionType::Broadcast;

            {
                let mut storage = test_storage()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                storage.insert(matching.stream_id.clone(), matching.clone());
                storage.insert(future.stream_id.clone(), future);
                storage.insert(broadcast.stream_id.clone(), broadcast);
            }

            let sessions = SipDialogSessionRepository::find_active_by_media_ssrc_before(
                "session-1",
                "media-1",
                "1100000001",
                at(2_000),
                at(1_500),
            )
            .await
            .expect("lookup unique dialog");
            assert_eq!(sessions, vec![matching.clone()]);

            let mut duplicate = matching;
            duplicate.stream_id = "unknown-duplicate".into();
            duplicate.created_at = at(900);
            duplicate.updated_at = at(900);
            test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(duplicate.stream_id.clone(), duplicate);

            let sessions = SipDialogSessionRepository::find_active_by_media_ssrc_before(
                "session-1",
                "media-1",
                "1100000001",
                at(2_000),
                at(1_500),
            )
            .await
            .expect("lookup ambiguous dialogs");
            assert_eq!(sessions.len(), 2);
        });
    }

    #[test]
    fn validation_rejects_invalid_enum_cseq_timestamp_and_route_values() {
        assert!("INVALID".parse::<DialogSessionType>().is_err());
        assert!("INVALID".parse::<DialogTransport>().is_err());
        assert!("INVALID".parse::<DialogState>().is_err());

        let mut session = inviting("invalid-stream");
        session.local_cseq = 0;
        assert!(session.validate().is_err());
        session.local_cseq = 1;
        session.updated_at = session.created_at - base::chrono::Duration::milliseconds(1);
        assert!(session.validate().is_err());
        session.updated_at = session.created_at;
        session.expire_at = session.last_seen_at;
        assert!(session.validate().is_err());
        session.expire_at = at(28_801_000);
        session.route_set = vec!["sip:proxy@127.0.0.1:5060\r\nRoute: sip:other".into()];
        assert!(session.validate().is_err());

        assert!(DialogState::Inviting.can_transition_to(DialogState::Terminated));
        assert!(DialogState::Inviting.can_transition_to(DialogState::Terminating));
        assert!(!DialogState::Terminated.can_transition_to(DialogState::Established));
    }

    #[test]
    fn inviting_dialog_can_enter_terminating_for_manual_cancel() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            SipDialogSessionRepository::insert_inviting(&inviting("manual-cancel"))
                .await
                .expect("insert inviting dialog");

            assert!(
                SipDialogSessionRepository::cas_transition(
                    "manual-cancel",
                    "session-1",
                    0,
                    DialogState::Inviting,
                    DialogState::Terminating,
                    at(1_001),
                )
                .await
                .expect("begin manual cancel")
            );
            let current = SipDialogSessionRepository::find_by_stream_id("manual-cancel")
                .await
                .expect("find dialog")
                .expect("dialog");
            assert_eq!(current.state, DialogState::Terminating);
            assert_eq!(current.version, 1);
        });
    }

    #[test]
    fn pages_twenty_thousand_owned_dialogs_without_duplicates() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            {
                let mut storage = test_storage()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for index in 0..20_000 {
                    let stream_id = format!("capacity-{index:05}");
                    storage.insert(stream_id.clone(), inviting(&stream_id));
                }
            }

            let mut cursor = None;
            let mut loaded = Vec::with_capacity(20_000);
            loop {
                let page = SipDialogSessionRepository::page_owned_by_states(
                    "session-1",
                    &[DialogState::Inviting],
                    cursor.as_deref(),
                    200,
                )
                .await
                .expect("page capacity rows");
                if page.is_empty() {
                    break;
                }
                cursor = page.last().map(|session| session.stream_id.clone());
                loaded.extend(page.into_iter().map(|session| session.stream_id));
            }

            assert_eq!(loaded.len(), 20_000);
            assert!(loaded.windows(2).all(|pair| pair[0] < pair[1]));
        });
    }

    #[test]
    fn terminal_metadata_history_and_retention_are_owner_scoped() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            let first = inviting("history-a");
            let mut other_owner = inviting("history-b");
            other_owner.signal_node_id = "session-2".into();
            SipDialogSessionRepository::insert_inviting(&first)
                .await
                .expect("insert first");
            SipDialogSessionRepository::insert_inviting(&other_owner)
                .await
                .expect("insert other owner");
            SipDialogSessionRepository::record_stop_reason("history-a", "session-1", "现场维护")
                .await
                .expect("record stop reason");
            SipDialogSessionRepository::record_stop_reason(
                "history-a",
                "session-1",
                "重复请求不得覆盖",
            )
            .await
            .expect("repeat stop reason");
            assert!(
                SipDialogSessionRepository::cas_mark_terminal(
                    "history-a",
                    "session-1",
                    0,
                    DialogState::Inviting,
                    DialogState::Orphan,
                    "invite_timeout",
                    Some("INVITE_TIMEOUT"),
                    at(2_000),
                )
                .await
                .expect("mark terminal")
            );
            let (history, total) = SipDialogSessionRepository::page_history_for_monitor(
                "session-1",
                1,
                20,
                &DialogMonitorFilter::default(),
            )
            .await
            .expect("history page");
            assert_eq!(total, 1);
            assert_eq!(
                history[0].terminal_reason.as_deref(),
                Some("invite_timeout")
            );
            assert_eq!(history[0].error_code.as_deref(), Some("INVITE_TIMEOUT"));
            assert_eq!(history[0].stop_reason.as_deref(), Some("现场维护"));
            assert_eq!(
                SipDialogSessionRepository::delete_terminal_before("session-1", at(3_000), 500)
                    .await
                    .expect("retention"),
                1
            );
            assert!(
                SipDialogSessionRepository::find_by_stream_id("history-b")
                    .await
                    .expect("other owner lookup")
                    .is_some()
            );
        });
    }

    #[test]
    fn active_monitor_filters_before_keyset_limit() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            for index in 0..5 {
                let mut session = inviting(&format!("monitor-{index}"));
                session.device_id = if index % 2 == 0 {
                    "device-a"
                } else {
                    "device-b"
                }
                .into();
                SipDialogSessionRepository::insert_inviting(&session)
                    .await
                    .expect("insert monitor row");
            }
            let rows = SipDialogSessionRepository::page_active_for_monitor(
                "session-1",
                Some("monitor-0"),
                2,
                &DialogMonitorFilter {
                    device_id: "device-a".into(),
                    ..DialogMonitorFilter::default()
                },
            )
            .await
            .expect("active monitor page");
            assert_eq!(
                rows.iter()
                    .map(|row| row.stream_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["monitor-2", "monitor-4"]
            );
        });
    }

    #[test]
    fn active_dialog_page_returns_filtered_total_and_offset_page() {
        let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let _guard = enable_dialog_test_storage();
            for index in 0..5 {
                let mut session = inviting(&format!("dialog-page-{index}"));
                session.device_id = if index % 2 == 0 {
                    "device-a"
                } else {
                    "device-b"
                }
                .into();
                SipDialogSessionRepository::insert_inviting(&session)
                    .await
                    .expect("insert active dialog");
            }
            let (rows, total) = SipDialogSessionRepository::page_active_dialogs_for_monitor(
                "session-1",
                2,
                2,
                &DialogMonitorFilter {
                    device_id: "device-a".into(),
                    state: "INVITING".into(),
                    ..DialogMonitorFilter::default()
                },
            )
            .await
            .expect("active dialog page");
            assert_eq!(total, 3);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].stream_id, "dialog-page-4");
        });
    }
}
