use std::sync::Arc;
use std::time::{Duration, Instant};

use base::chrono::{Duration as TimeDelta, Local};
use base::dashmap::DashSet;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::once_cell::sync::Lazy;
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
    NativeRuntimeFailure, NativeSubscriptionMetadata, SipResponseResult, SipRuntimeCache,
    recv_with_timeout,
};
use super::xml;

const SUBSCRIBE_WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const CATALOG_EVENT: &str = "Catalog";
const CATALOG_DEGRADED_MIN_FAILURES: u64 = 3;
const CATALOG_DEGRADED_AFTER: Duration = Duration::from_secs(120);

static DEGRADED_CATALOG_SUBSCRIPTIONS: Lazy<DashSet<String>> = Lazy::new(DashSet::new);

pub fn degraded_catalog_subscription_count() -> usize {
    DEGRADED_CATALOG_SUBSCRIPTIONS.len()
}

pub fn clear_degraded_catalog_subscription(device_id: &str) {
    DEGRADED_CATALOG_SUBSCRIPTIONS.remove(device_id);
}

pub async fn subscribe_catalog(device_id: &str, expires: u32) -> GlobalResult<()> {
    let expires = expires.max(1);
    match subscribe_catalog_once(device_id, expires, true).await {
        Ok(()) => {
            clear_degraded_catalog_subscription(device_id);
            Ok(())
        }
        Err(err) => {
            retry_new_catalog_subscription(
                device_id.to_string(),
                expires,
                "initial_subscribe_failed",
            );
            Err(err)
        }
    }
}

async fn subscribe_catalog_once(
    device_id: &str,
    expires: u32,
    log_failure: bool,
) -> GlobalResult<()> {
    let (host, port, base_protocol) = connected_target(device_id)?;
    let Some(session) = Register::get_connected_device_session(device_id) else {
        return Err(device_not_connected(device_id, log_failure));
    };
    let protocol = pjsip_protocol_from_base(base_protocol);
    let remote_target = target_uri(device_id, &host, port, protocol);
    let runtime = NativeSipRuntimeHandle::global()?;
    let operation_id = runtime.next_operation_id();
    let rx = SipRuntimeCache::global().insert_native_subscription_waiter(
        operation_id,
        NativeSubscriptionMetadata {
            device_id: device_id.to_string(),
            registration_epoch_id: session.registration_epoch_id.clone(),
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
        body: xml::encode_document(
            &catalog_subscription_body(device_id, expires),
            session.gb_version.as_deref(),
        )
        .to_vec(),
    };
    if let Err(err) = runtime.send_subscribe(&session.association, request) {
        SipRuntimeCache::global().remove_native_subscription_waiter(operation_id);
        return Err(err);
    }
    let response = recv_with_timeout(rx, SUBSCRIBE_WAIT_TIMEOUT)
        .await
        .map_err(|reason| {
            SipRuntimeCache::global().remove_native_subscription_waiter(operation_id);
            subscription_timeout(device_id, operation_id, reason, log_failure)
        })?
        .map_err(|failure| {
            subscription_runtime_failure(device_id, operation_id, failure, log_failure)
        })?;
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(subscription_rejected(
            device_id,
            response.status,
            log_failure,
        ))
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
        return Err(device_not_connected(device_id.as_ref(), true));
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
        body: xml::encode_document(
            &catalog_subscription_body(device_id.as_ref(), command.expires),
            session.gb_version.as_deref(),
        )
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
            subscription_timeout(device_id.as_ref(), operation_id, reason, true)
        })?;
    let response = match response {
        Ok(response) => response,
        Err(NativeRuntimeFailure::RuntimeNotFound(status)) => {
            Cache::catalog_subscription_remove(device_id.as_ref(), Some(generation));
            warn!(
                "catalog subscription state changed: state=rebuilding, previous_state=active, device_id={device_id}, reason=native_subscription_missing, pj_status={status}"
            );
            retry_new_catalog_subscription(
                device_id.to_string(),
                command.expires,
                "native_subscription_missing",
            );
            return Ok(());
        }
        Err(failure) => {
            Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
            schedule_catalog_retry(device_id.clone(), generation, command.expires);
            return Err(subscription_runtime_failure(
                device_id.as_ref(),
                operation_id,
                failure,
                true,
            ));
        }
    };
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
        retry_new_catalog_subscription(
            device_id.to_string(),
            command.expires,
            "device_subscription_missing",
        );
        Ok(())
    } else {
        Cache::catalog_subscription_mark_failed(device_id.as_ref(), generation);
        schedule_catalog_retry(device_id.clone(), generation, command.expires);
        Err(subscription_rejected(
            device_id.as_ref(),
            response.status,
            true,
        ))
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
            retry_new_catalog_subscription(
                device_id.to_string(),
                expires,
                "subscription_terminated",
            );
        }
    }
}

fn retry_new_catalog_subscription(device_id: String, expires: u32, trigger_reason: &'static str) {
    base::tokio::spawn(async move {
        let mut delay = Duration::from_secs(5);
        let mut failure_episode = FailureEpisode::default();
        let mut failure_started_at = None;
        let mut total_failures = 0_u64;
        loop {
            base::tokio::time::sleep(delay).await;
            if Register::get_connected_device_session(&device_id).is_none() {
                DEGRADED_CATALOG_SUBSCRIPTIONS.remove(&device_id);
                break;
            }
            match subscribe_catalog_once(&device_id, expires, false).await {
                Ok(()) => {
                    let was_degraded = DEGRADED_CATALOG_SUBSCRIPTIONS.remove(&device_id).is_some();
                    if let EpisodeDecision::Recovered {
                        total,
                        suppressed,
                        duration,
                    } = failure_episode.record_success(Instant::now())
                    {
                        base::log::info!(
                            "catalog subscription state changed: state=active, previous_state=retrying, outcome=recovered, device_id={device_id}, trigger_reason={trigger_reason}, was_degraded={was_degraded}, total_failures={total}, suppressed={suppressed}, duration_ms={}",
                            duration.as_millis()
                        );
                    } else {
                        base::log::info!(
                            "catalog subscription state changed: state=active, previous_state=rebuilding, outcome=recovered, device_id={device_id}, trigger_reason={trigger_reason}, was_degraded={was_degraded}"
                        );
                    }
                    break;
                }
                Err(err) => {
                    base::log::trace!(
                        "catalog subscription retry failed: device_id={device_id}, err={err}"
                    );
                    let now = Instant::now();
                    let started_at = *failure_started_at.get_or_insert(now);
                    total_failures = total_failures.saturating_add(1);
                    let newly_degraded =
                        should_mark_catalog_degraded(total_failures, started_at, now)
                            && DEGRADED_CATALOG_SUBSCRIPTIONS.insert(device_id.clone());
                    match failure_episode.record_failure(now) {
                        EpisodeDecision::Started => warn!(
                            "catalog subscription state changed: state=retrying, previous_state=rebuilding, device_id={device_id}, trigger_reason={trigger_reason}, reason=subscribe_failed"
                        ),
                        EpisodeDecision::Summary {
                            total,
                            since_last_summary,
                            suppressed,
                            duration,
                        } => warn!(
                            "catalog subscription remains unavailable: state={}, outcome=ongoing, device_id={device_id}, trigger_reason={trigger_reason}, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
                            if DEGRADED_CATALOG_SUBSCRIPTIONS.contains(&device_id) {
                                "degraded"
                            } else {
                                "retrying"
                            },
                            duration.as_millis()
                        ),
                        EpisodeDecision::Suppressed if newly_degraded => warn!(
                            "catalog subscription state changed: state=degraded, previous_state=retrying, outcome=ongoing, device_id={device_id}, trigger_reason={trigger_reason}, total_failures={total_failures}, duration_ms={}",
                            now.saturating_duration_since(started_at).as_millis()
                        ),
                        EpisodeDecision::Suppressed => {}
                        EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => {
                            unreachable!()
                        }
                    }
                    delay = Duration::from_secs(30);
                }
            }
        }
    });
}

fn should_mark_catalog_degraded(total_failures: u64, started_at: Instant, now: Instant) -> bool {
    total_failures >= CATALOG_DEGRADED_MIN_FAILURES
        && now.saturating_duration_since(started_at) >= CATALOG_DEGRADED_AFTER
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

fn subscription_timeout(
    device_id: &str,
    operation_id: u64,
    reason: &str,
    log_failure: bool,
) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::Timeout.code(),
        "device SUBSCRIBE response timeout",
        |msg| {
            if log_failure {
                error!(
                    "device_id={device_id}; operation_id={operation_id}; {msg}; reason={reason}"
                );
            }
        },
    )
}

fn subscription_runtime_failure(
    device_id: &str,
    operation_id: u64,
    failure: NativeRuntimeFailure,
    log_failure: bool,
) -> GlobalError {
    let stopped = failure == NativeRuntimeFailure::Stopped;
    GlobalError::new_sys_error("native SIP SUBSCRIBE failed", |msg| {
        if stopped {
            base::log::debug!(
                "device_id={device_id}; operation_id={operation_id}; action=subscribe; \
                 stage=native_runtime; outcome=local_cancelled; reason={failure}; {msg}"
            );
        } else if log_failure {
            error!(
                "device_id={device_id}; operation_id={operation_id}; action=subscribe; \
                 stage=native_runtime; outcome=failed; reason={failure}; {msg}"
            );
        }
    })
}

fn subscription_rejected(device_id: &str, status: u16, log_failure: bool) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "device rejected catalog subscription",
        |msg| {
            if log_failure {
                error!("device_id={device_id}; status={status}; {msg}");
            }
        },
    )
}

fn device_not_connected(device_id: &str, log_failure: bool) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::NotFound.code(),
        "device is not registered or connected",
        |msg| {
            if log_failure {
                error!("device_id={device_id}; {msg}");
            }
        },
    )
}

fn invalid_subscription(message: &'static str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidState.code(), message, |msg| {
        error!("{msg}")
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{catalog_refresh_delay, parse_subscription_state, should_mark_catalog_degraded};

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

    #[test]
    fn catalog_degradation_requires_failure_count_and_duration() {
        let started_at = Instant::now();
        assert!(!should_mark_catalog_degraded(
            2,
            started_at,
            started_at + Duration::from_secs(120)
        ));
        assert!(!should_mark_catalog_degraded(
            3,
            started_at,
            started_at + Duration::from_secs(119)
        ));
        assert!(should_mark_catalog_degraded(
            3,
            started_at,
            started_at + Duration::from_secs(120)
        ));
    }
}
