use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base::bytes::Bytes;
use gmv_pjsip::{
    AuthConfig, CleanupReport, SipAssociation, SipContext, SipEvent, SipLocalConfig, SipOutput,
    SipPacketMeta, SipTransportProtocol,
};

use super::bye::GbByeEvent;
use super::invite::{
    create_invite_play, create_invite_stop, GbIncomingInviteEvent, GbInviteAcceptedEvent,
    InvitePlayRequest, InviteStopRequest,
};
use super::message::{create_device_message, CreateDeviceMessageRequest, GbMessageEvent};
use super::register::GbRegisterEvent;

#[derive(Clone, Debug)]
pub struct GbSipConfig {
    pub platform_id: String,
    pub domain: String,
    pub realm: String,
    pub public_host: String,
    pub listen_port: u16,
    pub user_agent: String,
    pub default_expires: u32,
    pub transaction_ttl: Duration,
    pub auth: AuthConfig,
}

impl GbSipConfig {
    pub fn into_local_config(self) -> SipLocalConfig {
        SipLocalConfig {
            platform_id: self.platform_id,
            realm: self.realm,
            domain: self.domain,
            user_agent: self.user_agent,
            public_host: self.public_host,
            listen_port: self.listen_port,
            default_expires: self.default_expires,
            transaction_ttl: self.transaction_ttl,
            auth: self.auth,
        }
    }
}

#[derive(Clone)]
pub struct GbSipRuntime {
    ctx: Arc<SipContext>,
}

#[derive(Clone, Debug)]
pub struct GbSipRuntimeOutput {
    /// Packets that io.rs must send through the original association.
    pub sends: Vec<(SipAssociation, Bytes)>,
    /// Session-domain event. SIP header/context details are already handled by gmv_pjsip.
    pub event: Option<GbSipEvent>,
}

#[derive(Clone, Debug)]
pub enum GbSipEvent {
    Register(GbRegisterEvent),
    Message(GbMessageEvent),
    IncomingInvite(GbIncomingInviteEvent),
    InviteProceeding { call_id: String, status: u16 },
    InviteAccepted(GbInviteAcceptedEvent),
    InviteFailed { call_id: String, status: u16 },
    Ack { call_id: String },
    Bye(GbByeEvent),
    ByeConfirmed(GbByeEvent),
    Cancel { call_id: String },
}

impl From<SipEvent> for GbSipEvent {
    fn from(event: SipEvent) -> Self {
        match event {
            SipEvent::Register(e) => Self::Register(e.into()),
            SipEvent::Message(e) => Self::Message(e.into()),
            SipEvent::IncomingInvite(e) => Self::IncomingInvite(e.into()),
            SipEvent::InviteProceeding { call_id, status } => Self::InviteProceeding { call_id, status },
            SipEvent::InviteAccepted(e) => Self::InviteAccepted(e.into()),
            SipEvent::InviteFailed { call_id, status } => Self::InviteFailed { call_id, status },
            SipEvent::Ack(e) => Self::Ack { call_id: e.call_id },
            SipEvent::Bye(e) => Self::Bye(e.into()),
            SipEvent::ByeConfirmed(e) => Self::ByeConfirmed(e.into()),
            SipEvent::Cancel(e) => Self::Cancel { call_id: e.call_id },
        }
    }
}

impl GbSipRuntime {
    pub fn new(config: GbSipConfig) -> Self {
        Self {
            ctx: SipContext::new(config.into_local_config()),
        }
    }

    pub fn from_context(ctx: Arc<SipContext>) -> Self {
        Self { ctx }
    }

    pub fn context(&self) -> Arc<SipContext> {
        self.ctx.clone()
    }

    /// Entry point for io.rs after UDP/TCP SIP bytes are received.
    pub fn on_bytes(
        &self,
        bytes: Bytes,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        protocol: SipTransportProtocol,
    ) -> gmv_pjsip::Result<GbSipRuntimeOutput> {
        let meta = SipPacketMeta {
            local_addr,
            remote_addr,
            protocol,
            received_at: Instant::now(),
        };
        self.on_packet(bytes, meta)
    }

    pub fn on_packet(
        &self,
        bytes: Bytes,
        meta: SipPacketMeta,
    ) -> gmv_pjsip::Result<GbSipRuntimeOutput> {
        let output: SipOutput = self.ctx.handle_rx_packet(bytes, meta)?;
        Ok(GbSipRuntimeOutput {
            sends: output.sends,
            event: output.event.map(GbSipEvent::from),
        })
    }

    /// Platform -> device INVITE for live play/playback/talk-like custom SDP.
    pub fn create_invite_play(&self, req: InvitePlayRequest) -> gmv_pjsip::Result<Bytes> {
        create_invite_play(&self.ctx, req)
    }

    /// Platform -> device BYE.
    pub fn create_invite_stop(&self, req: InviteStopRequest) -> gmv_pjsip::Result<Bytes> {
        create_invite_stop(&self.ctx, req)
    }

    /// Platform -> device MESSAGE. Use this for Catalog/DeviceInfo/RecordInfo/PTZ/etc.
    pub fn create_device_message(&self, req: CreateDeviceMessageRequest) -> gmv_pjsip::Result<Bytes> {
        create_device_message(&self.ctx, req)
    }

    pub fn cleanup_expired(&self) -> CleanupReport {
        self.ctx.cleanup_expired()
    }

    pub fn cleanup_expired_with(&self, terminated_retain_for: Duration) -> CleanupReport {
        self.ctx.cleanup_expired_with(terminated_retain_for)
    }
}

/// Optional helper for business-layer dispatch. Projects that already have their
/// own event bus can match `GbSipEvent` directly and ignore this function.
pub fn dispatch_business_event(event: GbSipEvent) {
    match event {
        GbSipEvent::Register(e) => {
            if e.is_unregister() {
                // TODO: unregister device_id.
            } else {
                // TODO: refresh register store with e.device_id/e.association/e.contact.
            }
        }
        GbSipEvent::Message(e) => {
            let _ = e;
            // TODO: route Keepalive/Catalog/DeviceInfo/Alarm/RecordInfo by e.kind.
        }
        GbSipEvent::InviteAccepted(e) => {
            let _ = e;
            // TODO: bind call_id/stream_id/ssrc to media StreamSession and open RTP mapping.
        }
        GbSipEvent::Bye(e) | GbSipEvent::ByeConfirmed(e) => {
            let _ = e;
            // TODO: stop stream by call_id or stream_id.
        }
        GbSipEvent::IncomingInvite(e) => {
            let _ = e;
            // TODO: voice talk or device-initiated invite if enabled.
        }
        GbSipEvent::InviteProceeding { .. }
        | GbSipEvent::InviteFailed { .. }
        | GbSipEvent::Ack { .. }
        | GbSipEvent::Cancel { .. } => {}
    }
}
