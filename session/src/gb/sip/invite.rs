use gmv_pjsip::gb28181::sdp::SdpInfo;
use gmv_pjsip::{
    SipAssociation, SipDialogSnapshot, SipRuntimeEvent, SipRuntimeEventKind, SipTransportProtocol,
};

use crate::storage::dialog_session::DialogSessionType;

#[derive(Clone, Debug)]
pub struct InvitePlayRequest {
    pub device_id: String,
    pub channel_id: String,
    pub stream_id: String,
    pub media_node_id: String,
    pub session_type: DialogSessionType,
    pub device_host: String,
    pub device_port: u16,
    pub media_ip: String,
    pub media_port: u16,
    pub ssrc: u32,
    pub payload_type: u8,
    pub protocol: SipTransportProtocol,
    /// Optional custom SDP for playback/download/private extensions.
    /// When omitted, a standard GB28181 PS/RTP play SDP is generated.
    pub sdp: Option<String>,
    pub call_id: Option<String>,
    pub cseq: Option<u32>,
    pub subject: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InviteStopRequest {
    pub call_id: Option<String>,
    pub stream_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GbInviteAcceptedEvent {
    pub call_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub stream_id: String,
    pub ssrc: Option<u32>,
    pub dialog_snapshot: SipDialogSnapshot,
    pub remote_sdp: String,
    pub sdp_info: SdpInfo,
}

#[derive(Clone, Debug)]
pub struct GbIncomingInviteEvent {
    pub call_id: String,
    pub cseq: u32,
    pub association: SipAssociation,
    pub dialog_snapshot: SipDialogSnapshot,
    pub remote_sdp: String,
    pub from: String,
    pub to: String,
    pub subject: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AcceptBroadcastInviteRequest {
    pub device_id: String,
    pub channel_id: String,
    pub talk_id: String,
    pub media_node_id: String,
    pub media_ip: String,
    pub media_port: u16,
    pub ssrc: u32,
    pub payload_type: u8,
    pub invite: GbIncomingInviteEvent,
}

impl GbIncomingInviteEvent {
    pub fn from_native(event: &SipRuntimeEvent) -> Option<Self> {
        if event.kind != SipRuntimeEventKind::IncomingInvite {
            return None;
        }
        Some(Self {
            call_id: event.call_id.clone()?,
            cseq: event.cseq?,
            association: SipAssociation {
                local_addr: event.local_addr?,
                remote_addr: event.remote_addr?,
                protocol: event.protocol?,
            },
            dialog_snapshot: event.dialog_snapshot.clone()?,
            remote_sdp: String::from_utf8_lossy(&event.body).into_owned(),
            from: event.from_header.clone().unwrap_or_default(),
            to: event.to_header.clone().unwrap_or_default(),
            subject: event.subject.clone(),
        })
    }
}
