use std::sync::Arc;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use gmv_pjsip::SipOutboundSubscribe;
use gmv_pjsip::message::extract_uri;

use crate::gb::SessionConf;
use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, CatalogSubscriptionCommand};

use super::adapter::pjsip_protocol_from_base;
use super::command::connected_target;
use super::message::{GB_XML_CONTENT_TYPE, GbMessageEvent, target_uri};
use super::native_runtime::NativeSipRuntimeHandle;
use super::runtime_cache::{
    NativeSubscriptionMetadata, SipResponseResult, SipRuntimeCache, recv_with_timeout,
};
use super::xml;

const SUBSCRIBE_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const CATALOG_EVENT: &str = "Catalog";

pub async fn subscribe_catalog(device_id: &str, expires: u32) -> GlobalResult<()> {
    let expires = expires.max(1);
    match subscribe_catalog_once(device_id, expires).await {
        Ok(()) => Ok(()),
        Err(err) => {
            retry_new_catalog_subscription(device_id.to_string(), expires);
            Err(err)
        }
    }
}

async fn subscribe_catalog_once(device_id: &str, expires: u32) -> GlobalResult<()> {
    let (host, port, base_protocol) = connected_target(device_id)?;
    let Some(session) = Register::get_connected_device_session(device_id) else {
        return Err(device_not_connected(device_id));
    };
    let protocol = pjsip_protocol_from_base(base_protocol);
    let remote_target = target_uri(device_id, &host, port, protocol);
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global().insert_native_subscription_waiter(
        operation_id,
        NativeSubscriptionMetadata {
            device_id: device_id.to_string(),
            event: CATALOG_EVENT.to_string(),
            expires,
            remote_target: remote_target.clone(),
        },
        SUBSCRIBE_WAIT_TIMEOUT,
    );
    let conf = SessionConf::get_session_by_conf();
    let request = SipOutboundSubscribe {
        operation_id,
        association_id: 0,
        protocol,
        target_uri: remote_target,
        from_uri: format!("<sip:{}@{}>", conf.domain_id, conf.domain),
        contact_uri: format!(
            "<{}>",
            target_uri(
                &conf.domain_id,
                &conf.wan_ip.to_string(),
                conf.wan_port,
                protocol,
            )
        ),
        call_id: None,
        event: CATALOG_EVENT.to_string(),
        expires,
        content_type: GB_XML_CONTENT_TYPE.to_string(),
        body: xml::encode_document(&catalog_subscription_body(device_id, expires)).to_vec(),
    };
    if let Err(err) = runtime.send_subscribe(&session.association, request) {
        SipRuntimeCache::global().remove_native_subscription_waiter(operation_id);
        return Err(err);
    }
    let response = recv_with_timeout(rx, SUBSCRIBE_WAIT_TIMEOUT)
        .await
        .map_err(|reason| {
            SipRuntimeCache::global().remove_native_subscription_waiter(operation_id);
            subscription_timeout(device_id, operation_id, reason)
        })?;
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(subscription_rejected(device_id, response.status))
    }
}

pub async fn refresh_catalog_subscription(
    device_id: Arc<str>,
    generation: u64,
) -> GlobalResult<()> {
    let Some(command) = Cache::catalog_subscription_take_refresh(device_id.as_ref(), generation)
    else {
        return Ok(());
    };
    let Some(session) = Register::get_connected_device_session(device_id.as_ref()) else {
        Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
        return Err(device_not_connected(device_id.as_ref()));
    };
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global()
        .insert_native_response_waiter(operation_id, SUBSCRIBE_WAIT_TIMEOUT);
    let request = SipOutboundSubscribe {
        operation_id,
        association_id: 0,
        protocol: pjsip_protocol_from_base(session.association.protocol),
        target_uri: String::new(),
        from_uri: String::new(),
        contact_uri: String::new(),
        call_id: Some(command.call_id.clone()),
        event: command.event.clone(),
        expires: command.expires,
        content_type: GB_XML_CONTENT_TYPE.to_string(),
        body: xml::encode_document(&catalog_subscription_body(
            device_id.as_ref(),
            command.expires,
        ))
        .to_vec(),
    };
    if let Err(err) = runtime.send_subscribe(&session.association, request) {
        SipRuntimeCache::global().remove_native_response_waiter(operation_id);
        Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
        return Err(err);
    }
    let response = recv_with_timeout(rx, SUBSCRIBE_WAIT_TIMEOUT)
        .await
        .map_err(|reason| {
            SipRuntimeCache::global().remove_native_response_waiter(operation_id);
            Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
            schedule_catalog_retry(device_id.clone(), generation, command.expires);
            subscription_timeout(device_id.as_ref(), operation_id, reason)
        })?;
    complete_refresh(device_id, command, response)
}

fn complete_refresh(
    device_id: Arc<str>,
    command: CatalogSubscriptionCommand,
    response: SipResponseResult,
) -> GlobalResult<()> {
    let generation = command.generation;
    if (200..300).contains(&response.status) {
        match complete_catalog_subscription(
            device_id.as_ref(),
            generation,
            &command.remote_target,
            &command.from_header,
            &command.to_header,
            command.expires,
            response,
        ) {
            Ok(expires) => {
                schedule_catalog_refresh(device_id, generation, expires);
                Ok(())
            }
            Err(err) => {
                Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
                schedule_catalog_retry(device_id, generation, command.expires);
                Err(err)
            }
        }
    } else if response.status == 481 {
        Cache::catalog_subscription_remove(device_id.as_ref(), Some(generation));
        retry_new_catalog_subscription(device_id.to_string(), command.expires);
        Ok(())
    } else {
        Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
        schedule_catalog_retry(device_id.clone(), generation, command.expires);
        Err(subscription_rejected(device_id.as_ref(), response.status))
    }
}

pub fn accept_catalog_notify(event: &GbMessageEvent, device_id: &str) -> bool {
    let (Some(call_id), Some(event_header)) = (event.call_id.as_deref(), event.event.as_deref())
    else {
        return false;
    };
    let Some(generation) = Cache::catalog_subscription_validate_notify(
        device_id,
        call_id,
        event_header,
        event.from_tag.as_deref(),
        event.to_tag.as_deref(),
    ) else {
        return false;
    };

    if let Some(state) = event.subscription_state.as_deref() {
        let (state, expires) = parse_subscription_state(state);
        if state.eq_ignore_ascii_case("terminated") {
            terminate_catalog_subscription(device_id, generation);
        } else if let Some(expires) = expires {
            let expires = expires.max(1);
            Cache::catalog_subscription_update_expires(device_id, generation, expires);
            schedule_catalog_refresh(Arc::from(device_id), generation, expires);
        }
    }
    true
}

fn complete_catalog_subscription(
    device_id: &str,
    generation: u64,
    fallback_remote_target: &str,
    fallback_from_header: &str,
    fallback_to_header: &str,
    requested_expires: u32,
    response: SipResponseResult,
) -> GlobalResult<u32> {
    let metadata = response.metadata;
    let remote_target = metadata
        .contact
        .as_deref()
        .and_then(extract_uri)
        .unwrap_or_else(|| fallback_remote_target.to_string());
    let from_header = metadata
        .from_header
        .unwrap_or_else(|| fallback_from_header.to_string());
    let to_header = metadata
        .to_header
        .unwrap_or_else(|| fallback_to_header.to_string());
    let remote_tag = metadata.to_tag.unwrap_or_default();
    if !Cache::catalog_subscription_complete(
        device_id,
        generation,
        remote_target,
        Vec::new(),
        from_header,
        to_header,
        remote_tag,
    ) {
        return Err(invalid_subscription(
            "catalog subscription state changed before response",
        ));
    }
    let expires = metadata.expires.unwrap_or(requested_expires).max(1);
    Cache::catalog_subscription_update_expires(device_id, generation, expires);
    Ok(expires)
}

#[test]
fn test_catalog() {
    let body = catalog_subscription_body("asf", 3600);
    println!("{}", body);
}

fn catalog_subscription_body(device_id: &str, expires: u32) -> String {
    let now = Local::now();
    let end = now + TimeDelta::seconds(i64::from(expires));
    let sn = super::sequence::next_sn();
    xml::build_catalog_subscription(
        sn,
        device_id,
        &now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        &end.format("%Y-%m-%dT%H:%M:%S").to_string(),
    )
}

pub(super) fn schedule_catalog_refresh(device_id: Arc<str>, generation: u64, expires: u32) {
    let key = TimeScheduleKey::CatalogSubscription(device_id, generation);
    let _ = Register::scheduler().remove_register(&key);
    if let Err(err) = Register::scheduler().insert_register(key, catalog_refresh_delay(expires)) {
        warn!("schedule catalog subscription refresh failed: {err}");
    }
}

fn catalog_refresh_delay(expires: u32) -> Duration {
    let advance = (expires / 10).clamp(1, 30);
    Duration::from_secs(u64::from(expires.saturating_sub(advance).max(1)))
}

fn schedule_catalog_retry(device_id: Arc<str>, generation: u64, expires: u32) {
    let key = TimeScheduleKey::CatalogSubscription(device_id, generation);
    let _ = Register::scheduler().remove_register(&key);
    let delay = Duration::from_secs(u64::from(expires.clamp(1, 30)));
    if let Err(err) = Register::scheduler().insert_register(key, delay) {
        warn!("schedule catalog subscription retry failed: {err}");
    }
}

fn terminate_catalog_subscription(device_id: &str, generation: u64) {
    let expires = Cache::catalog_subscription_expires(device_id, generation);
    if Cache::catalog_subscription_remove(device_id, Some(generation)) {
        if let Some(expires) = expires {
            retry_new_catalog_subscription(device_id.to_string(), expires);
        }
    }
}

fn retry_new_catalog_subscription(device_id: String, expires: u32) {
    base::tokio::spawn(async move {
        let mut delay = Duration::from_secs(5);
        loop {
            base::tokio::time::sleep(delay).await;
            if Register::get_connected_device_session(&device_id).is_none() {
                break;
            }
            match subscribe_catalog_once(&device_id, expires).await {
                Ok(()) => break,
                Err(err) => {
                    warn!("retry catalog subscription failed: device_id={device_id}, err={err}");
                    delay = Duration::from_secs(30);
                }
            }
        }
    });
}

fn parse_subscription_state(value: &str) -> (&str, Option<u32>) {
    let mut parts = value.split(';').map(str::trim);
    let state = parts.next().unwrap_or_default();
    let expires = parts.find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.eq_ignore_ascii_case("expires")
            .then(|| value.trim().parse().ok())
            .flatten()
    });
    (state, expires)
}

fn subscription_timeout(device_id: &str, operation_id: u64, reason: &str) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::Timeout.code(),
        "device SUBSCRIBE response timeout",
        |msg| error!("device_id={device_id}; operation_id={operation_id}; {msg}; reason={reason}"),
    )
}

fn subscription_rejected(device_id: &str, status: u16) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected catalog subscription",
        |msg| error!("device_id={device_id}; status={status}; {msg}"),
    )
}

fn device_not_connected(device_id: &str) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::NotFound.code(),
        "device is not registered or connected",
        |msg| error!("device_id={device_id}; {msg}"),
    )
}

fn invalid_subscription(message: &'static str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidState.code(), message, |msg| {
        error!("{msg}")
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{catalog_refresh_delay, parse_subscription_state};

    #[test]
    fn schedules_catalog_refresh_before_native_refresh() {
        assert_eq!(catalog_refresh_delay(3_600), Duration::from_secs(3_570));
        assert_eq!(catalog_refresh_delay(300), Duration::from_secs(270));
        assert_eq!(catalog_refresh_delay(5), Duration::from_secs(4));
        assert_eq!(catalog_refresh_delay(1), Duration::from_secs(1));
    }

    #[test]
    fn catalog_refresh_builds_a_new_body() {
        let first = super::catalog_subscription_body("device", 3_600);
        let second = super::catalog_subscription_body("device", 3_600);

        assert_ne!(first, second);
    }

    #[test]
    fn parses_subscription_state_expires() {
        assert_eq!(
            parse_subscription_state("active;expires=3599"),
            ("active", Some(3599))
        );
        assert_eq!(
            parse_subscription_state("terminated;reason=timeout"),
            ("terminated", None)
        );
    }
}
