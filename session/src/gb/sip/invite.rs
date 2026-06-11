use base::bytes::Bytes;
use gmv_pjsip::gb28181::sdp::{PlaySdpOptions, SdpInfo, build_play_sdp};
use gmv_pjsip::{
    CreateBye, CreateInvite, CreateTalkInvite, IncomingInviteEvent, InviteAcceptedEvent,
    SipAssociation, SipContext, SipTransportProtocol, TalkAudioCodec, TalkSdpMode,
};

use super::message::target_uri;

#[derive(Clone, Debug)]
pub struct InvitePlayRequest {
    pub device_id: String,
    pub channel_id: String,
    pub stream_id: String,
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
pub struct InviteTalkRequest {
    pub device_id: String,
    pub channel_id: String,
    pub talk_id: String,
    pub device_host: String,
    pub device_port: u16,
    pub media_ip: String,
    pub media_port: u16,
    pub ssrc: u32,
    pub payload_type: u8,
    pub codec: TalkAudioCodec,
    pub mode: TalkSdpMode,
    pub protocol: SipTransportProtocol,
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
    pub remote_contact: Option<String>,
    pub remote_sdp: String,
    pub sdp_info: SdpInfo,
}

impl From<InviteAcceptedEvent> for GbInviteAcceptedEvent {
    fn from(event: InviteAcceptedEvent) -> Self {
        Self {
            call_id: event.call_id,
            device_id: event.device_id,
            channel_id: event.channel_id,
            stream_id: event.stream_id,
            ssrc: event.ssrc,
            remote_contact: event.remote_contact,
            remote_sdp: event.remote_sdp,
            sdp_info: event.sdp_info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GbIncomingInviteEvent {
    pub call_id: String,
    pub association: SipAssociation,
    pub remote_sdp: String,
    pub from: String,
    pub to: String,
    pub subject: Option<String>,
}

impl From<IncomingInviteEvent> for GbIncomingInviteEvent {
    fn from(event: IncomingInviteEvent) -> Self {
        Self {
            call_id: event.call_id,
            association: event.association,
            remote_sdp: event.remote_sdp,
            from: event.from,
            to: event.to,
            subject: event.subject,
        }
    }
}

pub fn create_invite_play(ctx: &SipContext, req: InvitePlayRequest) -> gmv_pjsip::Result<Bytes> {
    let target_uri = target_uri(
        &req.device_id,
        &req.device_host,
        req.device_port,
        req.protocol,
    );
    let sdp = req.sdp.unwrap_or_else(|| {
        build_play_sdp(PlaySdpOptions {
            ip: req.media_ip,
            port: req.media_port,
            ssrc: req.ssrc,
            payload_type: req.payload_type,
        })
    });

    ctx.create_invite(CreateInvite {
        device_id: req.device_id,
        channel_id: req.channel_id,
        stream_id: req.stream_id,
        target_uri,
        sdp,
        ssrc: Some(req.ssrc),
        protocol: req.protocol,
        call_id: req.call_id,
        cseq: req.cseq,
        subject: req.subject,
    })
}

pub fn create_talk_invite(ctx: &SipContext, req: InviteTalkRequest) -> gmv_pjsip::Result<Bytes> {
    let target_uri = target_uri(
        &req.device_id,
        &req.device_host,
        req.device_port,
        req.protocol,
    );
    ctx.create_talk_invite(CreateTalkInvite {
        device_id: req.device_id,
        channel_id: req.channel_id,
        talk_id: req.talk_id,
        target_uri,
        media_ip: req.media_ip,
        media_port: req.media_port,
        ssrc: Some(req.ssrc),
        payload_type: req.payload_type,
        codec: req.codec,
        mode: req.mode,
        protocol: req.protocol,
        call_id: req.call_id,
        cseq: req.cseq,
        subject: req.subject,
    })
}

pub fn create_invite_stop(ctx: &SipContext, req: InviteStopRequest) -> gmv_pjsip::Result<Bytes> {
    ctx.create_bye(CreateBye {
        call_id: req.call_id,
        stream_id: req.stream_id,
    })
}
