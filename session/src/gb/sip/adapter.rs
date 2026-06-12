use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base::bytes::Bytes;
use base::chrono::{Duration as TimeDelta, Local};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::state::{Association, Protocol};
use gmv_pjsip::{
    AuthAlgorithm, AuthConfig, CleanupReport, PasswordProvider, SipAssociation, SipContext,
    SipEvent, SipLocalConfig, SipMethod, SipOutput, SipPacketMeta, SipTransportProtocol,
};

use crate::gb::SessionConf;
use crate::register::core::{DeviceSession, Register};
use crate::service::api_serv;
use crate::service::stream_close;
use crate::state::AlarmConf;
use crate::state::model::AlarmInfo;
use crate::state::session::Cache as GeneralCache;
use crate::storage::db_task::{self, DbTask};
use crate::storage::entity::GmvDevice;

use super::bye::GbByeEvent;
use super::invite::{
    GbIncomingInviteEvent, GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest,
    InviteTalkRequest, create_invite_play, create_invite_stop, create_talk_invite,
};
use super::message::{
    CreateDeviceMessageRequest, GbMessageEvent, GbMessageKind, create_device_message,
};
use super::register::GbRegisterEvent;
use super::runtime_cache::SipRuntimeCache;
use super::xml::KV2Model;

static GB_SIP_RUNTIME: OnceLock<GbSipRuntime> = OnceLock::new();

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
    pub fn from_session_conf(conf: &SessionConf, auth_provider: Arc<dyn PasswordProvider>) -> Self {
        let realm = conf.domain.clone();
        Self {
            platform_id: conf.domain_id.clone(),
            domain: conf.domain.clone(),
            realm: realm.clone(),
            public_host: conf.wan_ip.to_string(),
            listen_port: conf.wan_port,
            user_agent: "GMV-PJSIP/0.1".to_string(),
            default_expires: 3600,
            transaction_ttl: Duration::from_secs(32),
            auth: AuthConfig::digest(realm, auth_provider, AuthAlgorithm::Md5),
        }
    }

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
    InviteProceeding {
        call_id: String,
        status: u16,
    },
    InviteAccepted(GbInviteAcceptedEvent),
    InviteFailed {
        call_id: String,
        stream_id: String,
        status: u16,
    },
    InfoProceeding {
        call_id: String,
        cseq: u32,
        status: u16,
    },
    InfoAccepted {
        call_id: String,
        cseq: u32,
        status: u16,
    },
    InfoFailed {
        call_id: String,
        cseq: u32,
        status: u16,
    },
    Ack {
        call_id: String,
    },
    Bye(GbByeEvent),
    ByeConfirmed(GbByeEvent),
    Cancel {
        call_id: String,
    },
    StandardRequest(GbMessageEvent),
    StandardResponse {
        method: SipMethod,
        call_id: String,
        cseq: u32,
        status: u16,
        contact: Option<String>,
        record_routes: Vec<String>,
        from_header: Option<String>,
        to_header: Option<String>,
        to_tag: Option<String>,
        expires: Option<u32>,
    },
}

impl From<SipEvent> for GbSipEvent {
    fn from(event: SipEvent) -> Self {
        match event {
            SipEvent::Register(e) => Self::Register(e.into()),
            SipEvent::Message(e) => Self::Message(e.into()),
            SipEvent::IncomingInvite(e) => Self::IncomingInvite(e.into()),
            SipEvent::InviteProceeding { call_id, status } => {
                Self::InviteProceeding { call_id, status }
            }
            SipEvent::InviteAccepted(e) => Self::InviteAccepted(e.into()),
            SipEvent::InviteFailed {
                call_id,
                stream_id,
                status,
            } => Self::InviteFailed {
                call_id,
                stream_id,
                status,
            },
            SipEvent::InfoProceeding {
                call_id,
                cseq,
                status,
            } => Self::InfoProceeding {
                call_id,
                cseq,
                status,
            },
            SipEvent::InfoAccepted {
                call_id,
                cseq,
                status,
            } => Self::InfoAccepted {
                call_id,
                cseq,
                status,
            },
            SipEvent::InfoFailed {
                call_id,
                cseq,
                status,
            } => Self::InfoFailed {
                call_id,
                cseq,
                status,
            },
            SipEvent::Ack(e) => Self::Ack { call_id: e.call_id },
            SipEvent::Bye(e) => Self::Bye(e.into()),
            SipEvent::ByeConfirmed(e) => Self::ByeConfirmed(e.into()),
            SipEvent::Cancel(e) => Self::Cancel { call_id: e.call_id },
            SipEvent::StandardRequest(e) => {
                Self::StandardRequest(GbMessageEvent::from_standard_request(e))
            }
            SipEvent::StandardResponse(e) => Self::StandardResponse {
                method: e.method,
                call_id: e.call_id,
                cseq: e.cseq,
                status: e.status,
                contact: e.contact,
                record_routes: e.record_routes,
                from_header: e.from_header,
                to_header: e.to_header,
                to_tag: e.to_tag,
                expires: e.expires,
            },
        }
    }
}

impl GbSipRuntime {
    pub fn new(config: GbSipConfig) -> Self {
        Self {
            ctx: SipContext::new(config.into_local_config()),
        }
    }

    pub fn init_global(config: GbSipConfig) -> &'static Self {
        GB_SIP_RUNTIME.get_or_init(|| Self::new(config))
    }

    pub fn global() -> Option<&'static Self> {
        GB_SIP_RUNTIME.get()
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

    /// Platform -> device INVITE for live play/playback/custom SDP.
    pub fn create_invite_play(&self, req: InvitePlayRequest) -> gmv_pjsip::Result<Bytes> {
        create_invite_play(&self.ctx, req)
    }

    /// Platform -> device talk INVITE.
    pub fn create_talk_invite(&self, req: InviteTalkRequest) -> gmv_pjsip::Result<Bytes> {
        create_talk_invite(&self.ctx, req)
    }

    /// Platform -> device BYE.
    pub fn create_invite_stop(&self, req: InviteStopRequest) -> gmv_pjsip::Result<Bytes> {
        create_invite_stop(&self.ctx, req)
    }

    /// Platform -> device MESSAGE. Use this for Catalog/DeviceInfo/RecordInfo/PTZ/etc.
    pub fn create_device_message(
        &self,
        req: CreateDeviceMessageRequest,
    ) -> gmv_pjsip::Result<Bytes> {
        create_device_message(&self.ctx, req)
    }

    pub fn create_subscribe(&self, req: gmv_pjsip::CreateSubscribe) -> gmv_pjsip::Result<Bytes> {
        self.ctx.create_subscribe(req)
    }

    pub fn create_playback_seek_info(
        &self,
        stream_id: &str,
        seek_second: f64,
    ) -> gmv_pjsip::Result<Bytes> {
        self.ctx
            .create_playback_seek_info(gmv_pjsip::CreatePlaybackSeekInfo {
                call_id: None,
                stream_id: Some(stream_id.to_string()),
                seek_second,
                rtsp_cseq: None,
            })
    }

    pub fn create_playback_speed_info(
        &self,
        stream_id: &str,
        speed: f32,
    ) -> gmv_pjsip::Result<Bytes> {
        self.ctx
            .create_playback_speed_info(gmv_pjsip::CreatePlaybackSpeedInfo {
                call_id: None,
                stream_id: Some(stream_id.to_string()),
                scale: speed,
                range_start_second: None,
                rtsp_cseq: None,
            })
    }

    pub fn cleanup_expired(&self) -> CleanupReport {
        self.ctx.cleanup_expired()
    }

    pub fn cleanup_expired_with(&self, terminated_retain_for: Duration) -> CleanupReport {
        self.ctx.cleanup_expired_with(terminated_retain_for)
    }

    /// Apply SIP event effects to the existing GMV business/session stores.
    ///
    /// This is intentionally thin: gmv_pjsip owns SIP context; Register/Cache
    /// own GMV business state. The event is still returned to io.rs callers so
    /// other async business pipelines can consume it.
    pub fn apply_business_event(&self, event: &GbSipEvent) -> GlobalResult<()> {
        apply_business_event(event)
    }
}

pub fn pjsip_protocol_from_base(protocol: Protocol) -> SipTransportProtocol {
    match protocol {
        Protocol::TCP => SipTransportProtocol::Tcp,
        _ => SipTransportProtocol::Udp,
    }
}

pub fn base_protocol_from_pjsip(protocol: SipTransportProtocol) -> Protocol {
    match protocol {
        SipTransportProtocol::Tcp | SipTransportProtocol::Tls => Protocol::TCP,
        SipTransportProtocol::Udp => Protocol::UDP,
    }
}

pub fn pjsip_association_from_base(association: &Association) -> SipAssociation {
    SipAssociation {
        local_addr: association.local_addr,
        remote_addr: association.remote_addr,
        protocol: pjsip_protocol_from_base(association.protocol),
    }
}

pub fn base_association_from_pjsip(association: &SipAssociation) -> Association {
    Association::new(
        association.local_addr,
        association.remote_addr,
        base_protocol_from_pjsip(association.protocol),
    )
}

pub fn apply_business_event(event: &GbSipEvent) -> GlobalResult<()> {
    match event {
        GbSipEvent::Register(e) => apply_register_event(e),
        GbSipEvent::Message(e) => apply_message_event(e),
        GbSipEvent::Bye(e) | GbSipEvent::ByeConfirmed(e) => apply_bye_event(e),
        GbSipEvent::InviteAccepted(e) => {
            info!(
                "SIP INVITE accepted: call_id={}, device_id={}, channel_id={}, stream_id={}, ssrc={:?}",
                e.call_id, e.device_id, e.channel_id, e.stream_id, e.ssrc
            );
            SipRuntimeCache::global().complete_invite(e);
            Ok(())
        }
        GbSipEvent::IncomingInvite(e) => {
            info!(
                "incoming SIP INVITE: call_id={}, association={:?}",
                e.call_id, e.association
            );
            Ok(())
        }
        GbSipEvent::InviteProceeding { call_id, status } => {
            debug!("SIP INVITE proceeding: call_id={call_id}, status={status}");
            Ok(())
        }
        GbSipEvent::InviteFailed {
            call_id,
            stream_id,
            status,
        } => {
            warn!("SIP INVITE failed: call_id={call_id}, stream_id={stream_id}, status={status}");
            SipRuntimeCache::global().fail_invite(super::runtime_cache::SipInviteFailure {
                call_id: call_id.clone(),
                stream_id: stream_id.clone(),
                status: *status,
            });
            Ok(())
        }
        GbSipEvent::InfoProceeding {
            call_id,
            cseq,
            status,
        } => {
            debug!("SIP INFO proceeding: call_id={call_id}, cseq={cseq}, status={status}");
            Ok(())
        }
        GbSipEvent::InfoAccepted {
            call_id,
            cseq,
            status,
        } => {
            debug!("SIP INFO accepted: call_id={call_id}, cseq={cseq}, status={status}");
            SipRuntimeCache::global().complete_response(
                &SipMethod::Info,
                call_id,
                *cseq,
                *status,
                Default::default(),
            );
            Ok(())
        }
        GbSipEvent::InfoFailed {
            call_id,
            cseq,
            status,
        } => {
            warn!("SIP INFO failed: call_id={call_id}, cseq={cseq}, status={status}");
            SipRuntimeCache::global().complete_response(
                &SipMethod::Info,
                call_id,
                *cseq,
                *status,
                Default::default(),
            );
            Ok(())
        }
        GbSipEvent::Ack { call_id } => {
            debug!("SIP ACK received: call_id={call_id}");
            Ok(())
        }
        GbSipEvent::Cancel { call_id } => {
            warn!("SIP CANCEL received: call_id={call_id}");
            Ok(())
        }
        GbSipEvent::StandardRequest(e) => apply_message_event(e),
        GbSipEvent::StandardResponse {
            method,
            call_id,
            cseq,
            status,
            contact,
            record_routes,
            from_header,
            to_header,
            to_tag,
            expires,
        } => {
            debug!(
                "SIP standard response: method={method}, call_id={call_id}, cseq={cseq}, status={status}"
            );
            if matches!(method, &SipMethod::Bye) && *status == 481 {
                let event = GbByeEvent {
                    call_id: call_id.clone(),
                    stream_id: SipRuntimeCache::global().stream_id_by_call_id(call_id),
                    device_id: None,
                };
                return apply_bye_event(&event);
            }
            if matches!(method, &SipMethod::Bye) && *status >= 300 {
                SipRuntimeCache::global().fail_bye(call_id, *status);
                return Ok(());
            }
            SipRuntimeCache::global().complete_response(
                method,
                call_id,
                *cseq,
                *status,
                super::runtime_cache::SipResponseMetadata {
                    contact: contact.clone(),
                    record_routes: record_routes.clone(),
                    from_header: from_header.clone(),
                    to_header: to_header.clone(),
                    to_tag: to_tag.clone(),
                    expires: *expires,
                },
            );
            Ok(())
        }
    }
}

fn apply_bye_event(event: &GbByeEvent) -> GlobalResult<()> {
    info!(
        "SIP BYE event: call_id={}, stream_id={:?}, device_id={:?}",
        event.call_id, event.stream_id, event.device_id
    );
    let stream_id = event
        .stream_id
        .clone()
        .or_else(|| SipRuntimeCache::global().stream_id_by_call_id(&event.call_id));
    let waiter_completed = SipRuntimeCache::global().complete_bye(event);
    if let Some(stream_id) = stream_id.as_deref() {
        SipRuntimeCache::global().remove_stream_indexes(stream_id, Some(&event.call_id));
    }
    if !waiter_completed {
        let call_id = event.call_id.clone();
        base::tokio::spawn(async move {
            let _ = api_serv::peer_dialog_terminated(call_id).await;
        });
    }
    Ok(())
}

fn apply_register_event(event: &GbRegisterEvent) -> GlobalResult<()> {
    let device_id: Arc<str> = Arc::from(event.device_id.as_str());
    if event.is_unregister() {
        Register::remove_device(&device_id);
        GeneralCache::reset_device_state(&event.device_id);
        db_task::submit(DbTask::ExpireDeviceOnline {
            device_id: event.device_id.clone(),
        });
        return Ok(());
    }

    let oauth = super::auth::global()
        .and_then(|cache| cache.get_by_device(&event.device_id))
        .ok_or_else(|| {
            GlobalError::new_sys_error(
                &format!("registered device auth state missing: {}", event.device_id),
                |msg| error!("{msg}"),
            )
        })?;
    let association = base_association_from_pjsip(&event.association);
    let expires = event.expires.max(1);
    let heartbeat_sec = oauth.heartbeat_sec;
    let mut session = DeviceSession::build(
        event.contact.clone().unwrap_or_default(),
        association.clone(),
        heartbeat_sec,
        Duration::from_secs(u64::from(expires)),
    );
    session.set_gb_version(event.gb_version.clone());
    session.set_registration_identity(event.call_id.clone(), event.cseq);
    if event.support_lr {
        session.enable_lr();
    }
    Register::register_device(device_id, session)?;
    GeneralCache::catalog_subscription_remove(&event.device_id, None);

    let now = Local::now().naive_local();
    db_task::submit(DbTask::UpsertDevice(GmvDevice {
        device_id: event.device_id.clone(),
        transport: match event.association.protocol {
            SipTransportProtocol::Udp => "UDP",
            SipTransportProtocol::Tcp => "TCP",
            SipTransportProtocol::Tls => "TLS",
        }
        .to_string(),
        register_expires: expires,
        register_time: now,
        online_expire_time: Some(
            now + TimeDelta::seconds(i64::from(heartbeat_sec).saturating_mul(3)),
        ),
        local_addr: association.remote_addr.to_string(),
        contact_uri: event.contact.clone().unwrap_or_default(),
        enable_lr: u8::from(event.support_lr),
        gb_version: event.gb_version.clone(),
    }));

    let query_device_id = event.device_id.clone();
    let subscribe_expires = expires;
    base::tokio::spawn(async move {
        base::tokio::time::sleep(Duration::from_millis(1500)).await;
        let sn = Local::now()
            .timestamp()
            .unsigned_abs()
            .min(u64::from(u32::MAX)) as u32;
        if let Err(err) = super::command::query_device_info(&query_device_id, sn).await {
            warn!(
                "query device info after register failed: device_id={query_device_id}, err={err}"
            );
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(err) =
            super::command::query_catalog(&query_device_id, sn.saturating_add(1)).await
        {
            warn!("query catalog after register failed: device_id={query_device_id}, err={err}");
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(err) =
            super::subscription::subscribe_catalog(&query_device_id, subscribe_expires).await
        {
            warn!(
                "subscribe catalog after register failed: device_id={query_device_id}, err={err}"
            );
        }
    });
    Ok(())
}

fn apply_message_event(event: &GbMessageEvent) -> GlobalResult<()> {
    let items = super::xml::parse_items(&event.body)?;
    let device_id = event
        .device_id
        .as_deref()
        .or(event.xml_device_id.as_deref());

    match event.kind {
        GbMessageKind::Keepalive => {
            let Some(device_id) = device_id else {
                warn!("keepalive MESSAGE missing device id");
                return Ok(());
            };
            let association = base_association_from_pjsip(&event.association);
            Register::device_heart(&Arc::<str>::from(device_id), association)?;
            db_task::submit(DbTask::TouchDeviceHeartbeat {
                device_id: device_id.to_string(),
            });
        }
        GbMessageKind::DeviceInfo => {
            db_task::submit(DbTask::UpdateDeviceExtInfo(items));
        }
        GbMessageKind::Catalog => {
            if let Some(device_id) = device_id {
                if matches!(event.method.as_ref(), Some(SipMethod::Notify))
                    && !super::subscription::accept_catalog_notify(event, device_id)
                {
                    warn!(
                        "ignore catalog NOTIFY outside active subscription: device_id={device_id}, call_id={:?}",
                        event.call_id
                    );
                    return Ok(());
                }
                db_task::submit(DbTask::InsertDeviceCatalog {
                    device_id: device_id.to_string(),
                    items,
                });
            } else {
                warn!("catalog MESSAGE missing device id");
            }
        }
        GbMessageKind::Alarm => dispatch_alarm(device_id, items)?,
        GbMessageKind::MediaStatus => {
            let channel_id = super::xml::value(&items, super::xml::NOTIFY_DEVICE_ID);
            let notify_type = super::xml::value(&items, super::xml::NOTIFY_TYPE);
            if notify_type.is_none_or(|value| value == "121") {
                if let (Some(device_id), Some(channel_id)) = (device_id, channel_id) {
                    for stream_id in crate::state::session::Cache::stream_ids_for_media_status(
                        device_id, channel_id,
                    ) {
                        stream_close::begin(stream_id);
                    }
                }
            }
        }
        GbMessageKind::UploadSnapshotFinished | GbMessageKind::Notify => {
            if let Some(session_id) = event.snapshot_session_id.as_deref() {
                let key = crate::service::edge_serv::rebuild_snapshot_wait_key(session_id);
                if crate::state::session::Cache::notify_snapshot_wait(&key) {
                    info!("snapshot upload notification received: session_id={session_id}");
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn dispatch_alarm(device_id: Option<&str>, items: Vec<(String, String)>) -> GlobalResult<()> {
    let Some(device_id) = device_id else {
        warn!("alarm MESSAGE missing device id");
        return Ok(());
    };
    let conf = AlarmConf::get_alarm_conf();
    if !conf.enable {
        return Ok(());
    }
    let mut alarm = AlarmInfo::kv_to_model(items)?;
    alarm.deviceId = device_id.to_string();
    let Some(push_url) = conf.push_url.clone() else {
        return Ok(());
    };
    base::tokio::spawn(async move {
        use crate::http::client::HttpBiz;

        let result = async {
            let client = crate::http::client::HttpClient::template(&push_url)?;
            client
                .call_alarm_info(&alarm)
                .await
                .hand_log(|msg| error!("{msg}"))?;
            GlobalResult::<()>::Ok(())
        }
        .await;
        if let Err(err) = result {
            error!("push alarm failed: device_id={}, err={err}", alarm.deviceId);
        }
    });
    Ok(())
}

/// Optional helper for business-layer dispatch. Projects that already have their
/// own event bus can match `GbSipEvent` directly and ignore this function.
pub fn dispatch_business_event(event: GbSipEvent) {
    if let Err(err) = apply_business_event(&event) {
        error!("apply SIP business event failed: {err}");
    }
}
