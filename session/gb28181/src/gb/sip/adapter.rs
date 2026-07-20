use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::dashmap::DashMap;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::state::{Association, Protocol};
use base::tokio::sync::Semaphore;
use gmv_pjsip::{SipAssociation, SipMethod, SipTransportProtocol};

use crate::guard_integration::publish_guard_event;
use crate::register::core::{DeviceSession, Register};
use crate::service::{api_serv, stream_close};
use crate::state::session::Cache as GeneralCache;
use crate::state::{AlarmConf, model::AlarmInfo};
use crate::storage::db_task::{self, DbTask};
use crate::storage::entity::GmvDevice;

use super::bye::GbByeEvent;
use super::invite::GbIncomingInviteEvent;
use super::message::{GbMessageEvent, GbMessageKind};
use super::register::GbRegisterEvent;
use super::runtime_cache::SipRuntimeCache;
use super::xml::KV2Model;

const POST_ONLINE_SYNC_CONCURRENCY: usize = 32;
static POST_ONLINE_SYNC_LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
static POST_ONLINE_SYNC_DEVICES: OnceLock<DashMap<String, u32>> = OnceLock::new();

#[derive(Clone, Debug)]
pub enum GbSipEvent {
    Register(GbRegisterEvent),
    Message(GbMessageEvent),
    IncomingInvite(GbIncomingInviteEvent),
    Ack { call_id: String },
    Bye(GbByeEvent),
    Cancel { call_id: String },
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

pub fn base_association_from_pjsip(association: &SipAssociation) -> Association {
    Association::new(
        association.local_addr,
        association.remote_addr,
        base_protocol_from_pjsip(association.protocol),
    )
}

pub async fn apply_business_event(event: &GbSipEvent) -> GlobalResult<()> {
    match event {
        GbSipEvent::Register(event) => apply_register_event(event).await,
        GbSipEvent::Message(event) => apply_message_event(event),
        GbSipEvent::IncomingInvite(event) => {
            info!(
                "incoming SIP INVITE: call_id={}, association={:?}",
                event.call_id, event.association
            );
            Ok(())
        }
        GbSipEvent::Ack { call_id } => {
            debug!("SIP ACK received: call_id={call_id}");
            Ok(())
        }
        GbSipEvent::Bye(event) => apply_bye_event(event),
        GbSipEvent::Cancel { call_id } => {
            debug!("SIP CANCEL received: outcome=peer_cancelled, call_id={call_id}");
            Ok(())
        }
    }
}

fn apply_bye_event(event: &GbByeEvent) -> GlobalResult<()> {
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
            api_serv::peer_dialog_terminated(call_id).await;
        });
    } else {
        debug!(
            "SIP BYE completed pending waiter: call_id={}, outcome=expected_peer_termination",
            event.call_id
        );
    }
    Ok(())
}

async fn apply_register_event(event: &GbRegisterEvent) -> GlobalResult<()> {
    let device_id: Arc<str> = Arc::from(event.device_id.as_str());
    if event.is_unregister() {
        Register::remove_device(&device_id);
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
    let heartbeat_sec = oauth.heartbeat_sec_u8()?;
    let mut session = DeviceSession::build(
        event.contact.clone().unwrap_or_default(),
        association.clone(),
        heartbeat_sec,
        Duration::from_secs(u64::from(expires)),
    );
    session.set_gb_version(event.gb_version.clone());
    session.set_registration_identity(event.call_id.clone(), event.cseq);
    if Register::get_device_session(&event.device_id).is_none()
        && let Some(device) = GmvDevice::query_gmv_device_by_device_id(&event.device_id).await?
        && device.registration_epoch_closed_at.is_none()
    {
        let now = Local::now().naive_local();
        let registration_expire_at =
            device.register_time + TimeDelta::seconds(i64::from(device.register_expires));
        if registration_expire_at > now
            && device
                .online_expire_time
                .is_some_and(|online_expire_time| online_expire_time > now)
        {
            if device.registration_call_id.as_deref() == Some(&event.call_id)
                && device
                    .registration_cseq
                    .is_some_and(|cseq| cseq > i64::from(event.cseq))
            {
                warn!(
                    "stale REGISTER ignored from durable ordering snapshot: device_id={}, call_id={}, cseq={}",
                    event.device_id, event.call_id, event.cseq
                );
                return Ok(());
            }
            session.set_optional_registration_epoch_id(device.registration_epoch_id);
            session.mark_registration_snapshot_restored();
        }
    }
    if event.support_lr {
        session.enable_lr();
    }
    let outcome = Register::register_device(device_id, session)?;
    if matches!(
        outcome,
        crate::register::core::RegisterOutcome::Stale
            | crate::register::core::RegisterOutcome::Retransmission
    ) {
        return Ok(());
    }
    let current_session = Register::get_device_session(&event.device_id).ok_or_else(|| {
        GlobalError::new_sys_error("registered device session disappeared", |msg| {
            error!("{msg}")
        })
    })?;

    let now = Local::now().naive_local();
    GmvDevice {
        device_id: event.device_id.clone(),
        transport: match event.association.protocol {
            SipTransportProtocol::Udp => "UDP",
            SipTransportProtocol::Tcp => "TCP",
            SipTransportProtocol::Tls => "TLS",
        }
        .to_string(),
        register_expires: expires,
        register_time: now,
        registration_call_id: Some(event.call_id.clone()),
        registration_cseq: Some(i64::from(event.cseq)),
        registration_epoch_id: current_session.registration_epoch_id.clone(),
        registration_epoch_closed_at: None,
        online_expire_time: Some(
            now + TimeDelta::seconds(i64::from(heartbeat_sec).saturating_mul(3).saturating_add(1)),
        ),
        local_addr: association.remote_addr.to_string(),
        contact_uri: event.contact.clone().unwrap_or_default(),
        enable_lr: u8::from(event.support_lr),
        gb_version: event.gb_version.clone(),
    }
    .insert_single_gmv_device_by_register()
    .await?;

    if !Register::mark_registration_ready(
        &event.device_id,
        current_session.registration_epoch_id.as_deref(),
    ) {
        return Err(GlobalError::new_sys_error(
            "registered device session changed before persistence completed",
            |msg| error!("device_id={}; {msg}", event.device_id),
        ));
    }

    if outcome.needs_post_online_sync() {
        GeneralCache::catalog_subscription_remove(&event.device_id, None);
        schedule_post_online_sync(event.device_id.clone(), expires);
    }
    Ok(())
}

pub(crate) fn schedule_post_online_sync(device_id: String, expires: u32) {
    let pending = POST_ONLINE_SYNC_DEVICES.get_or_init(DashMap::new);
    if pending.insert(device_id.clone(), expires).is_some() {
        return;
    }
    let limit = POST_ONLINE_SYNC_LIMIT
        .get_or_init(|| Arc::new(Semaphore::new(POST_ONLINE_SYNC_CONCURRENCY)))
        .clone();
    base::tokio::spawn(async move {
        let Ok(_permit) = limit.acquire_owned().await else {
            pending.remove(&device_id);
            return;
        };
        base::tokio::time::sleep(Duration::from_millis(1500)).await;
        if !Register::has_session(&device_id) {
            pending.remove(&device_id);
            return;
        }
        if let Err(err) =
            super::command::query_device_info(&device_id, super::sequence::next_sn()).await
        {
            warn!("query device info after online failed: device_id={device_id}, err={err}");
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if !Register::has_session(&device_id) {
            pending.remove(&device_id);
            return;
        }
        if let Err(err) =
            super::command::query_catalog(&device_id, super::sequence::next_sn()).await
        {
            warn!("query catalog after online failed: device_id={device_id}, err={err}");
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if !Register::has_session(&device_id) {
            pending.remove(&device_id);
            return;
        }
        let current_expires = pending
            .remove(&device_id)
            .map(|(_, current)| current)
            .unwrap_or(expires);
        if let Err(err) = super::subscription::subscribe_catalog(&device_id, current_expires).await
        {
            warn!("subscribe catalog after online failed: device_id={device_id}, err={err}");
        }
    });
}

fn apply_message_event(event: &GbMessageEvent) -> GlobalResult<()> {
    let items = super::xml::parse_items(&event.body)?;
    let business_device_id = event
        .device_id
        .as_deref()
        .or(event.xml_device_id.as_deref());
    let source_device_id = event
        .source_device_id
        .as_deref()
        .or(event.device_id.as_deref());

    match event.kind {
        GbMessageKind::Keepalive => {
            let Some(device_id) = business_device_id else {
                warn!("keepalive MESSAGE missing device id");
                return Ok(());
            };
            Register::recover_device_on_keepalive(
                Arc::<str>::from(device_id),
                base_association_from_pjsip(&event.association),
            )?;
        }
        GbMessageKind::DeviceInfo => {
            let Some(_) = validate_message_source(event, source_device_id)? else {
                return Ok(());
            };
            db_task::submit(DbTask::UpdateDeviceExtInfo(items));
        }
        GbMessageKind::Catalog => {
            if let Some(device_id) = validate_message_source(event, source_device_id)? {
                if matches!(event.method.as_ref(), Some(SipMethod::Notify))
                    && !super::subscription::accept_catalog_notify(event, device_id)
                {
                    warn!(
                        "ignore catalog NOTIFY outside active subscription: \
                         device_id={device_id}, call_id={:?}",
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
        GbMessageKind::Alarm => {
            let Some(_) = validate_message_source(event, source_device_id)? else {
                return Ok(());
            };
            dispatch_alarm(business_device_id, items)?;
        }
        GbMessageKind::MediaStatus => {
            let Some(device_id) = validate_message_source(event, source_device_id)? else {
                return Ok(());
            };
            let channel_id = super::xml::value(&items, super::xml::NOTIFY_DEVICE_ID);
            let notify_type = super::xml::value(&items, super::xml::NOTIFY_TYPE);
            if notify_type.is_none_or(|value| value == "121") {
                if let Some(channel_id) = channel_id {
                    let candidates =
                        GeneralCache::stream_ids_for_media_status(device_id, channel_id);
                    match candidates.as_slice() {
                        [stream_id] => stream_close::begin(stream_id.clone()),
                        [] => debug!(
                            "MediaStatus has no active playback match: device_id={device_id}; channel_id={channel_id}"
                        ),
                        _ => warn!(
                            "MediaStatus playback match is ambiguous; keep sessions active: device_id={device_id}; channel_id={channel_id}; candidates={candidates:?}"
                        ),
                    }
                }
            }
        }
        GbMessageKind::Broadcast => {
            let Some(_) = validate_message_source(event, source_device_id)? else {
                return Ok(());
            };
            let sn = event
                .xml_sn
                .as_deref()
                .or_else(|| super::xml::value(&items, "Response,SN"));
            let target_id = event
                .xml_device_id
                .as_deref()
                .or_else(|| super::xml::value(&items, "Response,DeviceID"));
            let result = super::xml::value(&items, "Response,Result");
            if let (Some(sn), Some(target_id), Some(result)) = (sn, target_id, result) {
                SipRuntimeCache::global().complete_broadcast_response(
                    sn,
                    target_id,
                    result.eq_ignore_ascii_case("OK"),
                );
            }
        }
        GbMessageKind::UploadSnapshotFinished | GbMessageKind::Notify => {
            let Some(_) = validate_message_source(event, source_device_id)? else {
                return Ok(());
            };
            if let Some(session_id) = event.snapshot_session_id.as_deref() {
                let key = crate::service::edge_serv::rebuild_snapshot_wait_key(session_id);
                if GeneralCache::notify_snapshot_wait(&key) {
                    info!("snapshot upload notification received: session_id={session_id}");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_message_source<'a>(
    event: &GbMessageEvent,
    device_id: Option<&'a str>,
) -> GlobalResult<Option<&'a str>> {
    let Some(device_id) = device_id else {
        warn!(
            "ignore SIP business message without device id: kind={:?}, call_id={:?}",
            event.kind, event.call_id
        );
        return Ok(None);
    };
    Register::validate_registered_source(
        device_id,
        &base_association_from_pjsip(&event.association),
    )?;
    Ok(Some(device_id))
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
    let payload = base::serde_json::to_vec(&alarm).hand_log(|msg| error!("{msg}"))?;
    publish_guard_event("session.alarm", payload);
    Ok(())
}
