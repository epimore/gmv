use std::fmt::{Display, Formatter};
use std::str::FromStr;

use base::chrono::NaiveDateTime;
use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde_json;
use base::sqlx::{self, FromRow};

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};

const INSERT_COLUMNS: &str = "STREAM_ID,DEVICE_ID,CHANNEL_ID,SESSION_TYPE,\
SIGNAL_NODE_ID,MEDIA_NODE_ID,SSRC,CALL_ID,LOCAL_URI,REMOTE_URI,LOCAL_TAG,REMOTE_TAG,\
LOCAL_CSEQ,REMOTE_CSEQ,CONTACT_URI,ROUTE_SET,LOCAL_SIP_ADDR,REMOTE_SIP_ADDR,TRANSPORT,\
STATE,ESTABLISHED_AT,LAST_SEEN_AT,EXPIRE_AT,VERSION,CREATED_AT,UPDATED_AT";
const SELECT_COLUMNS: &str = "STREAM_ID AS stream_id,DEVICE_ID AS device_id,\
CHANNEL_ID AS channel_id,SESSION_TYPE AS session_type,SIGNAL_NODE_ID AS signal_node_id,\
MEDIA_NODE_ID AS media_node_id,SSRC AS ssrc,CALL_ID AS call_id,LOCAL_URI AS local_uri,\
REMOTE_URI AS remote_uri,LOCAL_TAG AS local_tag,REMOTE_TAG AS remote_tag,\
LOCAL_CSEQ AS local_cseq,REMOTE_CSEQ AS remote_cseq,CONTACT_URI AS contact_uri,\
ROUTE_SET AS route_set,LOCAL_SIP_ADDR AS local_sip_addr,REMOTE_SIP_ADDR AS remote_sip_addr,\
TRANSPORT AS transport,STATE AS state,ESTABLISHED_AT AS established_at,\
LAST_SEEN_AT AS last_seen_at,EXPIRE_AT AS expire_at,VERSION AS version,\
CREATED_AT AS created_at,UPDATED_AT AS updated_at";
const MAX_PAGE_SIZE: u32 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogSessionType {
    Live,
    Playback,
    Download,
    Talk,
}

impl Display for DialogSessionType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Live => "LIVE",
            Self::Playback => "PLAYBACK",
            Self::Download => "DOWNLOAD",
            Self::Talk => "TALK",
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
            "TALK" => Ok(Self::Talk),
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
                Self::Established | Self::Terminated | Self::Orphan
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
    pub device_id: String,
    pub channel_id: String,
    pub session_type: DialogSessionType,
    pub signal_node_id: String,
    pub media_node_id: String,
    pub ssrc: Option<String>,
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
        validate_len(&self.signal_node_id, 64, "signal_node_id")?;
        validate_len(&self.media_node_id, 64, "media_node_id")?;
        validate_optional_len(self.ssrc.as_deref(), 16, "ssrc")?;
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
    device_id: String,
    channel_id: String,
    session_type: String,
    signal_node_id: String,
    media_node_id: String,
    ssrc: Option<String>,
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
            device_id: row.device_id,
            channel_id: row.channel_id,
            session_type: row.session_type.parse()?,
            signal_node_id: row.signal_node_id,
            media_node_id: row.media_node_id,
            ssrc: row.ssrc,
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
        sqlx::query(&format!(
            "INSERT INTO GMV_SIP_DIALOG_SESSION ({INSERT_COLUMNS}) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        ))
        .bind(&session.stream_id)
        .bind(&session.device_id)
        .bind(&session.channel_id)
        .bind(session.session_type.to_string())
        .bind(&session.signal_node_id)
        .bind(&session.media_node_id)
        .bind(&session.ssrc)
        .bind(&session.call_id)
        .bind(&session.local_uri)
        .bind(&session.remote_uri)
        .bind(&session.local_tag)
        .bind(&session.remote_tag)
        .bind(session.local_cseq)
        .bind(session.remote_cseq)
        .bind(&session.contact_uri)
        .bind(route_set)
        .bind(&session.local_sip_addr)
        .bind(&session.remote_sip_addr)
        .bind(session.transport.to_string())
        .bind(session.state.to_string())
        .bind(session.established_at)
        .bind(session.last_seen_at)
        .bind(session.expire_at)
        .bind(session.version)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(())
    }

    pub async fn cas_mark_established(
        stream_id: &str,
        signal_node_id: &str,
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
        let result = sqlx::query(
            "UPDATE GMV_SIP_DIALOG_SESSION SET REMOTE_TAG=?,LOCAL_CSEQ=?,REMOTE_CSEQ=?,\
             CONTACT_URI=?,ROUTE_SET=?,LOCAL_SIP_ADDR=?,REMOTE_SIP_ADDR=?,STATE='ESTABLISHED',\
             ESTABLISHED_AT=?,LAST_SEEN_AT=?,EXPIRE_AT=?,UPDATED_AT=?,VERSION=VERSION+1 \
             WHERE STREAM_ID=? AND SIGNAL_NODE_ID=? AND STATE='INVITING' AND VERSION=? \
             AND CREATED_AT<=? AND UPDATED_AT<=?",
        )
        .bind(&fields.remote_tag)
        .bind(fields.local_cseq)
        .bind(fields.remote_cseq)
        .bind(&fields.contact_uri)
        .bind(route_set)
        .bind(&fields.local_sip_addr)
        .bind(&fields.remote_sip_addr)
        .bind(fields.established_at)
        .bind(fields.last_seen_at)
        .bind(fields.expire_at)
        .bind(fields.updated_at)
        .bind(stream_id)
        .bind(signal_node_id)
        .bind(expected_version)
        .bind(fields.established_at)
        .bind(fields.updated_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(result.rows_affected() == 1)
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

        let result = sqlx::query(
            "UPDATE GMV_SIP_DIALOG_SESSION SET LOCAL_CSEQ=?,STATE='TERMINATING',\
             UPDATED_AT=?,VERSION=VERSION+1 WHERE STREAM_ID=? AND SIGNAL_NODE_ID=? \
             AND STATE='ESTABLISHED' AND LOCAL_CSEQ=? AND VERSION=? AND UPDATED_AT<=?",
        )
        .bind(next_cseq)
        .bind(updated_at)
        .bind(stream_id)
        .bind(signal_node_id)
        .bind(current_cseq)
        .bind(expected_version)
        .bind(updated_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(result.rows_affected() == 1)
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

        let result = sqlx::query(
            "UPDATE GMV_SIP_DIALOG_SESSION SET STATE=?,UPDATED_AT=?,VERSION=VERSION+1 \
             WHERE STREAM_ID=? AND SIGNAL_NODE_ID=? AND STATE=? AND VERSION=? AND UPDATED_AT<=?",
        )
        .bind(next_state.to_string())
        .bind(updated_at)
        .bind(stream_id)
        .bind(signal_node_id)
        .bind(expected_state.to_string())
        .bind(expected_version)
        .bind(updated_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(result.rows_affected() == 1)
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

        let result = sqlx::query(
            "UPDATE GMV_SIP_DIALOG_SESSION SET LOCAL_CSEQ=?,UPDATED_AT=?,VERSION=VERSION+1 \
             WHERE STREAM_ID=? AND SIGNAL_NODE_ID=? AND STATE IN ('ESTABLISHED','TERMINATING') \
             AND LOCAL_CSEQ=? AND VERSION=? AND UPDATED_AT<=?",
        )
        .bind(next_cseq)
        .bind(updated_at)
        .bind(stream_id)
        .bind(signal_node_id)
        .bind(current_cseq)
        .bind(expected_version)
        .bind(updated_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(result.rows_affected() == 1)
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

        let result = sqlx::query(
            "UPDATE GMV_SIP_DIALOG_SESSION SET LAST_SEEN_AT=?,EXPIRE_AT=?,UPDATED_AT=?,\
             VERSION=VERSION+1 WHERE STREAM_ID=? AND SIGNAL_NODE_ID=? \
             AND STATE IN ('ESTABLISHED','TERMINATING') AND VERSION=? \
             AND LAST_SEEN_AT<=? AND UPDATED_AT<=?",
        )
        .bind(last_seen_at)
        .bind(expire_at)
        .bind(last_seen_at)
        .bind(stream_id)
        .bind(signal_node_id)
        .bind(expected_version)
        .bind(last_seen_at)
        .bind(last_seen_at)
        .execute(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        Ok(result.rows_affected() == 1)
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
        let row = sqlx::query_as::<_, SipDialogSessionRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM GMV_SIP_DIALOG_SESSION WHERE STREAM_ID=?"
        ))
        .bind(stream_id)
        .fetch_optional(get_conn_by_pool())
        .await
        .hand_log(|message| error!("{message}"))?;
        row.map(TryInto::try_into).transpose()
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
        let rows = sqlx::query_as::<_, SipDialogSessionRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM GMV_SIP_DIALOG_SESSION \
             WHERE CALL_ID=? ORDER BY STREAM_ID"
        ))
        .bind(call_id)
        .fetch_all(get_conn_by_pool())
        .await
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
                        && session.session_type != DialogSessionType::Talk
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

        let rows = sqlx::query_as::<_, SipDialogSessionRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM GMV_SIP_DIALOG_SESSION              WHERE SIGNAL_NODE_ID=? AND MEDIA_NODE_ID=? AND SSRC=?              AND SESSION_TYPE IN ('LIVE','PLAYBACK','DOWNLOAD')              AND STATE IN ('ESTABLISHED','TERMINATING')              AND CREATED_AT<=? AND EXPIRE_AT>?              ORDER BY CREATED_AT DESC LIMIT 2"
        ))
        .bind(signal_node_id)
        .bind(media_node_id)
        .bind(ssrc)
        .bind(first_seen_at)
        .bind(now)
        .fetch_all(get_conn_by_pool())
        .await
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

        let mut builder = sqlx::QueryBuilder::new(format!(
            "SELECT {SELECT_COLUMNS} FROM GMV_SIP_DIALOG_SESSION WHERE SIGNAL_NODE_ID="
        ));
        builder.push_bind(signal_node_id).push(" AND STATE IN (");
        let mut separated = builder.separated(",");
        for state in states {
            separated.push_bind(state.to_string());
        }
        separated.push_unseparated(")");
        if let Some(cursor) = after_stream_id {
            builder.push(" AND STREAM_ID>").push_bind(cursor);
        }
        builder.push(" ORDER BY STREAM_ID LIMIT ").push_bind(limit);
        let rows = builder
            .build_query_as::<SipDialogSessionRow>()
            .fetch_all(get_conn_by_pool())
            .await
            .hand_log(|message| error!("{message}"))?;
        rows.into_iter().map(TryInto::try_into).collect()
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
            device_id: "34020000001320000001".into(),
            channel_id: "34020000001320000101".into(),
            session_type: DialogSessionType::Playback,
            signal_node_id: "session-1".into(),
            media_node_id: "media-1".into(),
            ssrc: Some("1100000001".into()),
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
            last_seen_at: at(1_000),
            expire_at: at(28_801_000),
            version: 0,
            created_at: at(1_000),
            updated_at: at(1_000),
        }
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
                SipDialogSessionRepository::cas_transition(
                    "stream-1",
                    "session-1",
                    3,
                    DialogState::Terminating,
                    DialogState::Terminated,
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

            let mut talk = matching.clone();
            talk.stream_id = "unknown-talk".into();
            talk.session_type = DialogSessionType::Talk;

            {
                let mut storage = test_storage()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                storage.insert(matching.stream_id.clone(), matching.clone());
                storage.insert(future.stream_id.clone(), future);
                storage.insert(talk.stream_id.clone(), talk);
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
        assert!(!DialogState::Terminated.can_transition_to(DialogState::Established));
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
}
