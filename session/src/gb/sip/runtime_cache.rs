//! Runtime-only SIP waiters and lightweight indexes.
//!
//! This module replaces part of the old global rsip transaction cache at the
//! session layer. Protocol transaction/dialog state lives in `gmv_pjsip`; this
//! cache only contains business waiters that are needed by synchronous service
//! APIs, such as `play_live()` waiting for INVITE 200 OK.

use std::time::Duration;

use base::dashmap::DashMap;
use base::once_cell::sync::Lazy;
use base::tokio::sync::oneshot;
use base::tokio::time::{self, Instant};
use gmv_pjsip::SipDialogSnapshot;
use gmv_pjsip::SipMethod;
use gmv_pjsip::gb28181::sdp::SdpInfo;
use gmv_pjsip::message::{extract_tag, extract_uri};

use super::bye::GbByeEvent;
use super::invite::{GbIncomingInviteEvent, GbInviteAcceptedEvent};
use crate::state::session::Cache;

static SIP_RUNTIME_CACHE: Lazy<SipRuntimeCache> = Lazy::new(SipRuntimeCache::default);

#[derive(Default)]
pub struct SipRuntimeCache {
    invite_waiters: DashMap<String, InviteWaiter>,
    bye_waiters: DashMap<String, ByeWaiter>,
    response_waiters: DashMap<SipResponseKey, ResponseWaiter>,
    native_response_waiters: DashMap<u64, ResponseWaiter>,
    native_invite_waiters: DashMap<u64, NativeInviteWaiter>,
    native_subscription_waiters: DashMap<u64, NativeSubscriptionWaiter>,
    broadcast_response_waiters: DashMap<BroadcastResponseKey, BroadcastResponseWaiter>,
    broadcast_invite_waiters: DashMap<String, BroadcastInviteWaiter>,
    call_stream_index: DashMap<String, String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct BroadcastResponseKey {
    pub sn: String,
    pub target_id: String,
}

struct BroadcastResponseWaiter {
    deadline: Instant,
    tx: oneshot::Sender<bool>,
}

struct BroadcastInviteWaiter {
    deadline: Instant,
    source_id: String,
    tx: oneshot::Sender<GbIncomingInviteEvent>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct SipResponseKey {
    pub method: SipMethod,
    pub call_id: String,
    pub cseq: u32,
}

#[derive(Clone, Debug, Default)]
pub struct SipResponseMetadata {
    pub call_id: Option<String>,
    pub cseq: Option<u32>,
    pub event: Option<String>,
    pub contact: Option<String>,
    pub record_routes: Vec<String>,
    pub from_header: Option<String>,
    pub to_header: Option<String>,
    pub to_tag: Option<String>,
    pub expires: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct SipResponseResult {
    pub status: u16,
    pub metadata: SipResponseMetadata,
}

struct InviteWaiter {
    deadline: Instant,
    tx: oneshot::Sender<Result<GbInviteAcceptedEvent, SipInviteFailure>>,
}

#[derive(Clone, Debug)]
pub struct SipInviteFailure {
    pub call_id: String,
    pub stream_id: String,
    pub status: u16,
    pub dialog_established: bool,
}

struct ByeWaiter {
    deadline: Instant,
    tx: oneshot::Sender<Result<GbByeEvent, SipByeFailure>>,
}

#[derive(Clone, Debug)]
pub struct SipByeFailure {
    pub call_id: String,
    pub status: u16,
}

struct ResponseWaiter {
    deadline: Instant,
    tx: oneshot::Sender<SipResponseResult>,
}

#[derive(Clone, Debug)]
pub struct NativeInviteMetadata {
    pub device_id: String,
    pub channel_id: String,
    pub stream_id: String,
    pub ssrc: Option<u32>,
}

struct NativeInviteWaiter {
    deadline: Instant,
    metadata: NativeInviteMetadata,
    tx: oneshot::Sender<Result<GbInviteAcceptedEvent, SipInviteFailure>>,
}

#[derive(Clone, Debug)]
pub struct NativeSubscriptionMetadata {
    pub device_id: String,
    pub event: String,
    pub expires: u32,
    pub remote_target: String,
}

struct NativeSubscriptionWaiter {
    deadline: Instant,
    metadata: NativeSubscriptionMetadata,
    tx: oneshot::Sender<SipResponseResult>,
}

impl SipRuntimeCache {
    pub fn global() -> &'static Self {
        &SIP_RUNTIME_CACHE
    }

    pub fn insert_broadcast_response_waiter(
        &self,
        key: BroadcastResponseKey,
        ttl: Duration,
    ) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.broadcast_response_waiters.insert(
            key,
            BroadcastResponseWaiter {
                deadline: Instant::now() + ttl,
                tx,
            },
        );
        rx
    }

    pub fn complete_broadcast_response(&self, sn: &str, target_id: &str, accepted: bool) -> bool {
        self.broadcast_response_waiters
            .remove(&BroadcastResponseKey {
                sn: sn.to_string(),
                target_id: target_id.to_string(),
            })
            .map(|(_, waiter)| waiter.tx.send(accepted).is_ok())
            .unwrap_or(false)
    }

    pub fn remove_broadcast_response_waiter(&self, key: &BroadcastResponseKey) {
        self.broadcast_response_waiters.remove(key);
    }

    pub fn insert_broadcast_invite_waiter(
        &self,
        target_id: String,
        source_id: String,
        ttl: Duration,
    ) -> oneshot::Receiver<GbIncomingInviteEvent> {
        let (tx, rx) = oneshot::channel();
        self.broadcast_invite_waiters.insert(
            target_id,
            BroadcastInviteWaiter {
                deadline: Instant::now() + ttl,
                source_id,
                tx,
            },
        );
        rx
    }

    pub fn complete_broadcast_invite(&self, event: &GbIncomingInviteEvent) -> bool {
        let Some(source_id) = sip_user(&event.to) else {
            return false;
        };
        let from_id = sip_user(&event.from);
        let subject_ids = event
            .subject
            .as_deref()
            .map(broadcast_subject_ids)
            .unwrap_or_default();
        let target_id = self.broadcast_invite_waiters.iter().find_map(|item| {
            let target_id = item.key();
            (item.source_id == source_id
                && (from_id.as_deref() == Some(target_id.as_str())
                    || subject_ids.iter().any(|id| id == target_id)))
            .then(|| target_id.clone())
        });
        let Some(target_id) = target_id else {
            return false;
        };
        self.broadcast_invite_waiters
            .remove(&target_id)
            .map(|(_, waiter)| waiter.tx.send(event.clone()).is_ok())
            .unwrap_or(false)
    }

    pub fn remove_broadcast_invite_waiter(&self, target_id: &str) {
        self.broadcast_invite_waiters.remove(target_id);
    }

    pub fn insert_invite_waiter(
        &self,
        stream_id: String,
        ttl: Duration,
    ) -> oneshot::Receiver<Result<GbInviteAcceptedEvent, SipInviteFailure>> {
        let (tx, rx) = oneshot::channel();
        self.invite_waiters.insert(
            stream_id,
            InviteWaiter {
                deadline: Instant::now() + ttl,
                tx,
            },
        );
        rx
    }

    pub fn complete_invite(&self, event: &GbInviteAcceptedEvent) -> bool {
        self.call_stream_index
            .insert(event.call_id.clone(), event.stream_id.clone());
        self.invite_waiters
            .remove(&event.stream_id)
            .map(|(_, waiter)| waiter.tx.send(Ok(event.clone())).is_ok())
            .unwrap_or(false)
    }

    pub fn fail_invite(&self, failure: SipInviteFailure) -> bool {
        self.invite_waiters
            .remove(&failure.stream_id)
            .map(|(_, waiter)| waiter.tx.send(Err(failure)).is_ok())
            .unwrap_or(false)
    }

    pub fn insert_bye_waiter(
        &self,
        key: String,
        ttl: Duration,
    ) -> oneshot::Receiver<Result<GbByeEvent, SipByeFailure>> {
        let (tx, rx) = oneshot::channel();
        self.bye_waiters.insert(
            key,
            ByeWaiter {
                deadline: Instant::now() + ttl,
                tx,
            },
        );
        rx
    }

    pub fn complete_bye(&self, event: &GbByeEvent) -> bool {
        let keys = [Some(event.call_id.clone()), event.stream_id.clone()];
        for key in keys.into_iter().flatten() {
            if let Some((_, waiter)) = self.bye_waiters.remove(&key) {
                let _ = waiter.tx.send(Ok(event.clone()));
                return true;
            }
        }
        false
    }

    pub fn remove_bye_waiter(&self, key: &str) {
        self.bye_waiters.remove(key);
    }

    pub fn fail_bye(&self, call_id: &str, status: u16) -> bool {
        self.bye_waiters
            .remove(call_id)
            .map(|(_, waiter)| {
                waiter
                    .tx
                    .send(Err(SipByeFailure {
                        call_id: call_id.to_string(),
                        status,
                    }))
                    .is_ok()
            })
            .unwrap_or(false)
    }

    pub fn insert_response_waiter(
        &self,
        key: SipResponseKey,
        ttl: Duration,
    ) -> oneshot::Receiver<SipResponseResult> {
        let (tx, rx) = oneshot::channel();
        self.response_waiters.insert(
            key,
            ResponseWaiter {
                deadline: Instant::now() + ttl,
                tx,
            },
        );
        rx
    }

    pub fn complete_response(
        &self,
        method: &SipMethod,
        call_id: &str,
        cseq: u32,
        status: u16,
        metadata: SipResponseMetadata,
    ) -> bool {
        if status < 200 {
            return false;
        }
        let key = SipResponseKey {
            method: method.clone(),
            call_id: call_id.to_string(),
            cseq,
        };
        self.response_waiters
            .remove(&key)
            .map(|(_, waiter)| {
                waiter
                    .tx
                    .send(SipResponseResult { status, metadata })
                    .is_ok()
            })
            .unwrap_or(false)
    }

    pub fn remove_response_waiter(&self, key: &SipResponseKey) {
        self.response_waiters.remove(key);
    }

    pub fn insert_native_response_waiter(
        &self,
        operation_id: u64,
        ttl: Duration,
    ) -> oneshot::Receiver<SipResponseResult> {
        let (tx, rx) = oneshot::channel();
        self.native_response_waiters.insert(
            operation_id,
            ResponseWaiter {
                deadline: Instant::now() + ttl,
                tx,
            },
        );
        rx
    }

    pub fn complete_native_response(
        &self,
        operation_id: u64,
        status: u16,
        metadata: SipResponseMetadata,
    ) -> bool {
        if status < 200 {
            return false;
        }
        self.native_response_waiters
            .remove(&operation_id)
            .map(|(_, waiter)| {
                waiter
                    .tx
                    .send(SipResponseResult { status, metadata })
                    .is_ok()
            })
            .unwrap_or(false)
    }

    pub fn remove_native_response_waiter(&self, operation_id: u64) {
        self.native_response_waiters.remove(&operation_id);
    }

    pub fn insert_native_subscription_waiter(
        &self,
        operation_id: u64,
        metadata: NativeSubscriptionMetadata,
        ttl: Duration,
    ) -> oneshot::Receiver<SipResponseResult> {
        let (tx, rx) = oneshot::channel();
        self.native_subscription_waiters.insert(
            operation_id,
            NativeSubscriptionWaiter {
                deadline: Instant::now() + ttl,
                metadata,
                tx,
            },
        );
        rx
    }

    pub fn complete_native_subscription(
        &self,
        operation_id: u64,
        status: u16,
        response: SipResponseMetadata,
    ) -> bool {
        if status < 200 {
            return false;
        }
        self.native_subscription_waiters
            .remove(&operation_id)
            .map(|(_, waiter)| {
                let status = if (200..300).contains(&status) {
                    self.establish_native_subscription(&waiter.metadata, &response)
                        .then_some(status)
                        .unwrap_or(500)
                } else {
                    status
                };
                waiter
                    .tx
                    .send(SipResponseResult {
                        status,
                        metadata: response,
                    })
                    .is_ok()
            })
            .unwrap_or(false)
    }

    fn establish_native_subscription(
        &self,
        pending: &NativeSubscriptionMetadata,
        response: &SipResponseMetadata,
    ) -> bool {
        let (Some(call_id), Some(cseq), Some(from_header), Some(to_header)) = (
            response.call_id.clone(),
            response.cseq,
            response.from_header.clone(),
            response.to_header.clone(),
        ) else {
            return false;
        };
        let Some(local_tag) = extract_tag(&from_header) else {
            return false;
        };
        let remote_target = response
            .contact
            .as_deref()
            .and_then(extract_uri)
            .unwrap_or_else(|| pending.remote_target.clone());
        let Some(generation) = Cache::catalog_subscription_begin(
            pending.device_id.clone(),
            call_id,
            cseq,
            pending.event.clone(),
            pending.expires,
            remote_target.clone(),
            from_header.clone(),
            to_header.clone(),
            local_tag,
        ) else {
            return true;
        };
        let completed = Cache::catalog_subscription_complete(
            &pending.device_id,
            generation,
            remote_target,
            Vec::new(),
            from_header,
            to_header.clone(),
            extract_tag(&to_header).unwrap_or_default(),
        );
        if completed {
            Cache::catalog_subscription_update_expires(
                &pending.device_id,
                generation,
                response.expires.unwrap_or(pending.expires).max(1),
            );
        } else {
            Cache::catalog_subscription_remove(&pending.device_id, Some(generation));
        }
        completed
    }

    pub fn remove_native_subscription_waiter(&self, operation_id: u64) {
        self.native_subscription_waiters.remove(&operation_id);
    }

    pub fn insert_native_invite_waiter(
        &self,
        operation_id: u64,
        metadata: NativeInviteMetadata,
        ttl: Duration,
    ) -> oneshot::Receiver<Result<GbInviteAcceptedEvent, SipInviteFailure>> {
        let (tx, rx) = oneshot::channel();
        self.native_invite_waiters.insert(
            operation_id,
            NativeInviteWaiter {
                deadline: Instant::now() + ttl,
                metadata,
                tx,
            },
        );
        rx
    }

    pub fn complete_native_invite(
        &self,
        operation_id: u64,
        call_id: String,
        status: u16,
        remote_sdp: String,
        dialog_snapshot: Option<SipDialogSnapshot>,
    ) -> bool {
        if status < 200 {
            return false;
        }
        self.native_invite_waiters
            .remove(&operation_id)
            .map(|(_, waiter)| {
                let result = if (200..300).contains(&status) {
                    let Some(dialog_snapshot) = dialog_snapshot else {
                        let _ = waiter.tx.send(Err(SipInviteFailure {
                            call_id,
                            stream_id: waiter.metadata.stream_id,
                            status: 500,
                            dialog_established: true,
                        }));
                        return true;
                    };
                    let event = GbInviteAcceptedEvent {
                        call_id: call_id.clone(),
                        device_id: waiter.metadata.device_id,
                        channel_id: waiter.metadata.channel_id,
                        stream_id: waiter.metadata.stream_id,
                        ssrc: waiter.metadata.ssrc,
                        dialog_snapshot,
                        sdp_info: SdpInfo::parse_lossy(&remote_sdp),
                        remote_sdp,
                    };
                    self.call_stream_index
                        .insert(call_id, event.stream_id.clone());
                    Ok(event)
                } else {
                    Err(SipInviteFailure {
                        call_id,
                        stream_id: waiter.metadata.stream_id,
                        status,
                        dialog_established: false,
                    })
                };
                waiter.tx.send(result).is_ok()
            })
            .unwrap_or(false)
    }

    pub fn fail_native_invite(&self, operation_id: u64, status: u16) -> bool {
        self.native_invite_waiters
            .remove(&operation_id)
            .map(|(_, waiter)| {
                waiter
                    .tx
                    .send(Err(SipInviteFailure {
                        call_id: String::new(),
                        stream_id: waiter.metadata.stream_id,
                        status,
                        dialog_established: false,
                    }))
                    .is_ok()
            })
            .unwrap_or(false)
    }

    pub fn remove_native_invite_waiter(&self, operation_id: u64) {
        self.native_invite_waiters.remove(&operation_id);
    }

    pub fn fail_all_native(&self, status: u16) -> usize {
        let response_ids = self
            .native_response_waiters
            .iter()
            .map(|item| *item.key())
            .collect::<Vec<_>>();
        let subscription_ids = self
            .native_subscription_waiters
            .iter()
            .map(|item| *item.key())
            .collect::<Vec<_>>();
        let invite_ids = self
            .native_invite_waiters
            .iter()
            .map(|item| *item.key())
            .collect::<Vec<_>>();
        let mut failed = 0;
        for operation_id in response_ids {
            failed += usize::from(self.complete_native_response(
                operation_id,
                status,
                SipResponseMetadata::default(),
            ));
        }
        for operation_id in subscription_ids {
            failed += usize::from(self.complete_native_subscription(
                operation_id,
                status,
                SipResponseMetadata::default(),
            ));
        }
        for operation_id in invite_ids {
            failed += usize::from(self.fail_native_invite(operation_id, status));
        }
        failed
    }

    pub fn stream_id_by_call_id(&self, call_id: &str) -> Option<String> {
        self.call_stream_index
            .get(call_id)
            .map(|item| item.value().clone())
    }

    pub fn remove_stream_indexes(&self, stream_id: &str, call_id: Option<&str>) {
        self.invite_waiters.remove(stream_id);
        self.bye_waiters.remove(stream_id);
        if let Some(call_id) = call_id {
            self.call_stream_index.remove(call_id);
            self.bye_waiters.remove(call_id);
        }
    }

    pub fn restore_stream_index(&self, call_id: String, stream_id: String) {
        self.call_stream_index.insert(call_id, stream_id);
    }

    pub fn cleanup_expired(&self) -> RuntimeCleanupReport {
        let now = Instant::now();
        let mut invite_waiters = 0;
        let mut bye_waiters = 0;
        let mut response_waiters = 0;
        let mut native_response_waiters = 0;

        let expired_invites = self
            .invite_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| item.key().clone()))
            .collect::<Vec<_>>();
        for key in expired_invites {
            if self.invite_waiters.remove(&key).is_some() {
                invite_waiters += 1;
            }
        }
        let expired_native_invites = self
            .native_invite_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| *item.key()))
            .collect::<Vec<_>>();
        for operation_id in expired_native_invites {
            if self.native_invite_waiters.remove(&operation_id).is_some() {
                invite_waiters += 1;
            }
        }

        let expired_byes = self
            .bye_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| item.key().clone()))
            .collect::<Vec<_>>();
        for key in expired_byes {
            if self.bye_waiters.remove(&key).is_some() {
                bye_waiters += 1;
            }
        }

        let expired_responses = self
            .response_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| item.key().clone()))
            .collect::<Vec<_>>();
        for key in expired_responses {
            if self.response_waiters.remove(&key).is_some() {
                response_waiters += 1;
            }
        }
        let expired_native_responses = self
            .native_response_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| *item.key()))
            .collect::<Vec<_>>();
        for operation_id in expired_native_responses {
            if self.native_response_waiters.remove(&operation_id).is_some() {
                native_response_waiters += 1;
            }
        }
        let expired_native_subscriptions = self
            .native_subscription_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| *item.key()))
            .collect::<Vec<_>>();
        for operation_id in expired_native_subscriptions {
            if self
                .native_subscription_waiters
                .remove(&operation_id)
                .is_some()
            {
                native_response_waiters += 1;
            }
        }

        let expired_broadcast_responses = self
            .broadcast_response_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| item.key().clone()))
            .collect::<Vec<_>>();
        for key in expired_broadcast_responses {
            self.broadcast_response_waiters.remove(&key);
        }
        let expired_broadcast_invites = self
            .broadcast_invite_waiters
            .iter()
            .filter_map(|item| (item.deadline <= now).then(|| item.key().clone()))
            .collect::<Vec<_>>();
        for key in expired_broadcast_invites {
            self.broadcast_invite_waiters.remove(&key);
        }

        RuntimeCleanupReport {
            invite_waiters,
            bye_waiters,
            response_waiters,
            native_response_waiters,
        }
    }
}

fn sip_user(header: &str) -> Option<String> {
    let uri = extract_uri(header)?;
    let value = uri
        .strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))?;
    Some(value.split('@').next()?.to_string())
}

fn broadcast_subject_ids(subject: &str) -> Vec<String> {
    subject
        .split(',')
        .filter_map(|leg| leg.trim().split_once(':').map(|(id, _)| id.trim()))
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeCleanupReport {
    pub invite_waiters: usize,
    pub bye_waiters: usize,
    pub response_waiters: usize,
    pub native_response_waiters: usize,
}

pub async fn recv_with_timeout<T>(
    rx: oneshot::Receiver<T>,
    timeout: Duration,
) -> Result<T, &'static str> {
    match time::timeout(timeout, rx).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err("waiter closed"),
        Err(_) => Err("timeout"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming_broadcast_invite(
        from: &str,
        to: &str,
        subject: Option<&str>,
    ) -> GbIncomingInviteEvent {
        let local_addr = "127.0.0.1:5060".parse().expect("local addr");
        let remote_addr = "127.0.0.1:5061".parse().expect("remote addr");
        GbIncomingInviteEvent {
            call_id: "broadcast-call".into(),
            cseq: 20,
            association: gmv_pjsip::SipAssociation {
                local_addr,
                remote_addr,
                protocol: gmv_pjsip::SipTransportProtocol::Udp,
            },
            dialog_snapshot: SipDialogSnapshot {
                call_id: "broadcast-call".into(),
                local_uri: "sip:source@127.0.0.1:5060".into(),
                remote_uri: "sip:target@127.0.0.1:5061".into(),
                local_tag: "local".into(),
                remote_tag: "remote".into(),
                local_cseq: 1,
                remote_target: "sip:target@127.0.0.1:5061".into(),
                route_set: Vec::new(),
                protocol: gmv_pjsip::SipTransportProtocol::Udp,
                association_id: 0,
                local_addr,
                remote_addr,
            },
            remote_sdp: String::new(),
            from: from.into(),
            to: to.into(),
            subject: subject.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn broadcast_waiters_require_exact_sn_target_and_sip_users() {
        let cache = SipRuntimeCache::default();
        let ttl = Duration::from_secs(1);
        let key = BroadcastResponseKey {
            sn: "100".into(),
            target_id: "target".into(),
        };
        let mut response = cache.insert_broadcast_response_waiter(key, ttl);
        assert!(!cache.complete_broadcast_response("101", "target", true));
        assert!(cache.complete_broadcast_response("100", "target", true));
        assert!(response.try_recv().expect("broadcast response"));

        let mut invite =
            cache.insert_broadcast_invite_waiter("target".into(), "source".into(), ttl);
        assert!(!cache.complete_broadcast_invite(&incoming_broadcast_invite(
            "<sip:target@127.0.0.1>",
            "<sip:other@127.0.0.1>",
            None,
        )));
        assert!(cache.complete_broadcast_invite(&incoming_broadcast_invite(
            "<sip:device@127.0.0.1>",
            "<sip:source@127.0.0.1>",
            Some("source:03d7a8ef,target:0552354c"),
        )));
        assert_eq!(
            invite.try_recv().expect("broadcast invite").call_id,
            "broadcast-call"
        );
    }

    #[test]
    fn fail_all_native_completes_pending_waiters() {
        let cache = SipRuntimeCache::default();
        let ttl = Duration::from_secs(1);
        let mut response = cache.insert_native_response_waiter(1, ttl);
        let mut subscription = cache.insert_native_subscription_waiter(
            2,
            NativeSubscriptionMetadata {
                device_id: "device".into(),
                event: "Catalog".into(),
                expires: 3600,
                remote_target: "sip:device@127.0.0.1:5060".into(),
            },
            ttl,
        );
        let mut invite = cache.insert_native_invite_waiter(
            3,
            NativeInviteMetadata {
                device_id: "device".into(),
                channel_id: "channel".into(),
                stream_id: "stream".into(),
                ssrc: Some(1),
            },
            ttl,
        );

        assert_eq!(cache.fail_all_native(503), 3);
        assert_eq!(
            response.try_recv().expect("response completion").status,
            503
        );
        assert_eq!(
            subscription
                .try_recv()
                .expect("subscription completion")
                .status,
            503
        );
        assert_eq!(
            invite
                .try_recv()
                .expect("invite completion")
                .expect_err("invite failure")
                .status,
            503
        );
    }
}
