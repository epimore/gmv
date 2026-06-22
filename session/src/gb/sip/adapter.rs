use std::sync::Arc;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::state::{Association, Protocol};
use gmv_pjsip::{SipAssociation, SipMethod, SipTransportProtocol};

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

pub fn apply_business_event(event: GbSipEvent) -> GlobalResult<()> {
    match event {
        GbSipEvent::Register(event) => apply_register_event(&event),
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
        GbSipEvent::Bye(event) => apply_bye_event(&event),
        GbSipEvent::Cancel { call_id } => {
            warn!("SIP CANCEL received: call_id={call_id}");
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
    base::tokio::spawn(async move {
        base::tokio::time::sleep(Duration::from_millis(1500)).await;
        if let Err(err) =
            super::command::query_device_info(&query_device_id, super::sequence::next_sn()).await
        {
            warn!(
                "query device info after register failed: device_id={query_device_id}, err={err}"
            );
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(err) =
            super::command::query_catalog(&query_device_id, super::sequence::next_sn()).await
        {
            warn!("query catalog after register failed: device_id={query_device_id}, err={err}");
        }
        base::tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(err) = super::subscription::subscribe_catalog(&query_device_id, expires).await {
            warn!(
                "subscribe catalog after register failed: device_id={query_device_id}, err={err}"
            );
        }
    });
    Ok(())
}

fn apply_message_event(mut event: GbMessageEvent) -> GlobalResult<()> {
    let device_id = event
        .device_id
        .clone()
        .or_else(|| event.xml_device_id.clone());
    let device_id = device_id.as_deref();

    match event.kind {
        GbMessageKind::Keepalive => {
            let Some(device_id) = device_id else {
                warn!("keepalive MESSAGE missing device id");
                return Ok(());
            };
            Register::recover_device_on_keepalive(
                Arc::<str>::from(device_id),
                base_association_from_pjsip(&event.association),
            )?;
        }
        GbMessageKind::DeviceInfo => {
            db_task::submit(DbTask::UpdateDeviceExtInfo(std::mem::take(
                &mut event.items,
            )));
        }
        GbMessageKind::Catalog => {
            if let Some(device_id) = device_id {
                if matches!(event.method.as_ref(), Some(SipMethod::Notify))
                    && !super::subscription::accept_catalog_notify(&event, device_id)
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
                    items: std::mem::take(&mut event.items),
                });
            } else {
                warn!("catalog MESSAGE missing device id");
            }
        }
        GbMessageKind::Alarm => dispatch_alarm(device_id, std::mem::take(&mut event.items))?,
        GbMessageKind::MediaStatus => {
            let channel_id = super::xml::value(&event.items, super::xml::NOTIFY_DEVICE_ID);
            let notify_type = super::xml::value(&event.items, super::xml::NOTIFY_TYPE);
            if notify_type.is_none_or(|value| value == "121") {
                if let (Some(device_id), Some(channel_id)) = (device_id, channel_id) {
                    for stream_id in
                        GeneralCache::stream_ids_for_media_status(device_id, channel_id)
                    {
                        stream_close::begin(stream_id);
                    }
                }
            }
        }
        GbMessageKind::Broadcast => {
            let sn = event
                .xml_sn
                .as_deref()
                .or_else(|| super::xml::value(&event.items, "Response,SN"));
            let target_id = event
                .xml_device_id
                .as_deref()
                .or_else(|| super::xml::value(&event.items, "Response,DeviceID"));
            let result = super::xml::value(&event.items, "Response,Result");
            if let (Some(sn), Some(target_id), Some(result)) = (sn, target_id, result) {
                SipRuntimeCache::global().complete_broadcast_response(
                    sn,
                    target_id,
                    result.eq_ignore_ascii_case("OK"),
                );
            }
        }
        GbMessageKind::UploadSnapshotFinished | GbMessageKind::Notify => {
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
