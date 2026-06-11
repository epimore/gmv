use std::sync::Arc;
use std::time::Duration;

use base::bytes::Bytes;
use base::chrono::{Duration as TimeDelta, Local};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use gmv_pjsip::CreateSubscribe;
use gmv_pjsip::message::{HeaderMapExt, extract_uri};
use gmv_pjsip::parser::parse_sip_message;

use crate::register::core::{Register, TimeScheduleKey};
use crate::state::session::{Cache, CatalogSubscriptionCommand};

use super::command::{connected_target, runtime, send_request_and_wait_status, to_global_error};
use super::message::{GB_XML_CONTENT_TYPE, GbMessageEvent};
use super::runtime_cache::SipResponseResult;
use super::{pjsip_protocol_from_base, xml};

struct SubscribeRequestState {
    call_id: String,
    cseq: u32,
    event: String,
    remote_target: String,
    from_header: String,
    to_header: String,
    local_tag: String,
}

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
    let (host, port, protocol) = connected_target(device_id)?;
    let target_uri =
        super::message::target_uri(device_id, &host, port, pjsip_protocol_from_base(protocol));
    let event = format!(
        "Catalog;id={}",
        Local::now().timestamp_millis().unsigned_abs()
    );
    let body = catalog_subscription_body(device_id, expires);
    let bytes = runtime()?
        .create_subscribe(CreateSubscribe {
            target_uri: target_uri.clone(),
            body: xml::encode_document(&body),
            content_type: GB_XML_CONTENT_TYPE.to_string(),
            protocol: pjsip_protocol_from_base(protocol),
            call_id: None,
            cseq: None,
            event,
            expires,
            from_header: None,
            to_header: None,
            route_set: Vec::new(),
        })
        .map_err(to_global_error)?;
    let request = subscribe_request_state(&bytes)?;
    let Some(generation) = Cache::catalog_subscription_begin(
        device_id.to_string(),
        request.call_id,
        request.cseq,
        request.event,
        expires,
        request.remote_target.clone(),
        request.from_header.clone(),
        request.to_header.clone(),
        request.local_tag.clone(),
    ) else {
        return Ok(());
    };

    match send_request_and_wait_status(device_id, bytes).await {
        Ok(response) if (200..300).contains(&response.status) => {
            let completed = complete_catalog_subscription(
                device_id,
                generation,
                &request.remote_target,
                &request.from_header,
                &request.to_header,
                expires,
                response,
            );
            match completed {
                Ok(expires) => {
                    schedule_catalog_refresh(Arc::from(device_id), generation, expires);
                    Ok(())
                }
                Err(err) => {
                    Cache::catalog_subscription_remove(device_id, Some(generation));
                    Err(err)
                }
            }
        }
        Ok(response) => {
            Cache::catalog_subscription_remove(device_id, Some(generation));
            Err(subscription_rejected(device_id, response.status))
        }
        Err(err) => {
            Cache::catalog_subscription_remove(device_id, Some(generation));
            Err(err)
        }
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
    let bytes = build_refresh_request(device_id.as_ref(), &command)?;
    match send_request_and_wait_status(device_id.as_ref(), bytes).await {
        Ok(response) if (200..300).contains(&response.status) => {
            let completed = complete_catalog_subscription(
                device_id.as_ref(),
                generation,
                &command.remote_target,
                &command.from_header,
                &command.to_header,
                command.expires,
                response,
            );
            match completed {
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
        }
        Ok(response) if response.status == 481 => {
            Cache::catalog_subscription_remove(device_id.as_ref(), Some(generation));
            retry_new_catalog_subscription(device_id.to_string(), command.expires);
            Ok(())
        }
        Ok(response) => {
            Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
            schedule_catalog_retry(device_id.clone(), generation, command.expires);
            Err(subscription_rejected(device_id.as_ref(), response.status))
        }
        Err(err) => {
            Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
            schedule_catalog_retry(device_id, generation, command.expires);
            Err(err)
        }
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
            update_catalog_subscription_from_notify(device_id, generation, expires);
        }
    }
    true
}

fn build_refresh_request(
    device_id: &str,
    command: &CatalogSubscriptionCommand,
) -> GlobalResult<Bytes> {
    let (_, _, protocol) = connected_target(device_id)?;
    runtime()?
        .create_subscribe(CreateSubscribe {
            target_uri: command.remote_target.clone(),
            body: xml::encode_document(&catalog_subscription_body(device_id, command.expires)),
            content_type: GB_XML_CONTENT_TYPE.to_string(),
            protocol: pjsip_protocol_from_base(protocol),
            call_id: Some(command.call_id.clone()),
            cseq: Some(command.seq),
            event: command.event.clone(),
            expires: command.expires,
            from_header: Some(command.from_header.clone()),
            to_header: Some(command.to_header.clone()),
            route_set: command.route_set.clone(),
        })
        .map_err(to_global_error)
}

fn subscribe_request_state(bytes: &Bytes) -> GlobalResult<SubscribeRequestState> {
    let message = parse_sip_message(bytes.clone()).map_err(to_global_error)?;
    let cseq = message.cseq().map_err(to_global_error)?;
    Ok(SubscribeRequestState {
        call_id: message.call_id().map_err(to_global_error)?,
        cseq: cseq.number,
        event: required_header(&message, "Event")?,
        remote_target: message
            .request_uri()
            .ok_or_else(|| invalid_subscription("SUBSCRIBE missing request URI"))?
            .to_string(),
        from_header: required_header(&message, "From")?,
        to_header: required_header(&message, "To")?,
        local_tag: message
            .from_tag()
            .ok_or_else(|| invalid_subscription("SUBSCRIBE missing From tag"))?,
    })
}

fn required_header(message: &gmv_pjsip::SipMessage, name: &'static str) -> GlobalResult<String> {
    message
        .required_header(name)
        .map(str::to_string)
        .map_err(to_global_error)
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
    let route_set = metadata.record_routes.into_iter().rev().collect();
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
        route_set,
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

fn catalog_subscription_body(device_id: &str, expires: u32) -> String {
    let now = Local::now();
    let end = now + TimeDelta::seconds(i64::from(expires));
    let sn = now.timestamp().unsigned_abs().min(u64::from(u32::MAX)) as u32;
    xml::build_catalog_subscription(
        sn,
        device_id,
        &now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        &end.format("%Y-%m-%dT%H:%M:%S").to_string(),
    )
}

fn catalog_refresh_delay(expires: u32) -> Duration {
    let margin = (expires / 10).clamp(1, 60);
    Duration::from_secs(u64::from(expires.saturating_sub(margin).max(1)))
}

fn schedule_catalog_refresh(device_id: Arc<str>, generation: u64, expires: u32) {
    schedule_catalog(
        device_id,
        generation,
        catalog_refresh_delay(expires),
        "refresh",
    );
}

fn schedule_catalog_retry(device_id: Arc<str>, generation: u64, expires: u32) {
    schedule_catalog(
        device_id,
        generation,
        Duration::from_secs(u64::from(expires.clamp(1, 30))),
        "retry",
    );
}

fn schedule_catalog(device_id: Arc<str>, generation: u64, delay: Duration, action: &str) {
    let key = TimeScheduleKey::CatalogSubscription(device_id, generation);
    let _ = Register::scheduler().remove_register(&key);
    if let Err(err) = Register::scheduler().insert_register(key, delay) {
        warn!("schedule catalog subscription {action} failed: {err}");
    }
}

fn update_catalog_subscription_from_notify(device_id: &str, generation: u64, expires: u32) {
    let expires = expires.max(1);
    if Cache::catalog_subscription_update_expires(device_id, generation, expires) {
        schedule_catalog_refresh(Arc::from(device_id), generation, expires);
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

fn subscription_rejected(device_id: &str, status: u16) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected catalog subscription",
        |msg| error!("device_id={device_id}; status={status}; {msg}"),
    )
}

fn invalid_subscription(message: &'static str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidState.code(), message, |msg| {
        error!("{msg}")
    })
}

#[cfg(test)]
mod tests {
    use super::parse_subscription_state;

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
