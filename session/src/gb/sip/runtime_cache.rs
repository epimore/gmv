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
use gmv_pjsip::SipMethod;

use super::bye::GbByeEvent;
use super::invite::GbInviteAcceptedEvent;

static SIP_RUNTIME_CACHE: Lazy<SipRuntimeCache> = Lazy::new(SipRuntimeCache::default);

#[derive(Default)]
pub struct SipRuntimeCache {
    invite_waiters: DashMap<String, InviteWaiter>,
    bye_waiters: DashMap<String, ByeWaiter>,
    response_waiters: DashMap<SipResponseKey, ResponseWaiter>,
    call_stream_index: DashMap<String, String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct SipResponseKey {
    pub method: SipMethod,
    pub call_id: String,
    pub cseq: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct SipResponseResult {
    pub status: u16,
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

impl SipRuntimeCache {
    pub fn global() -> &'static Self {
        &SIP_RUNTIME_CACHE
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
            .map(|(_, waiter)| waiter.tx.send(SipResponseResult { status }).is_ok())
            .unwrap_or(false)
    }

    pub fn remove_response_waiter(&self, key: &SipResponseKey) {
        self.response_waiters.remove(key);
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

    pub fn cleanup_expired(&self) -> RuntimeCleanupReport {
        let now = Instant::now();
        let mut invite_waiters = 0;
        let mut bye_waiters = 0;
        let mut response_waiters = 0;

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

        RuntimeCleanupReport {
            invite_waiters,
            bye_waiters,
            response_waiters,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeCleanupReport {
    pub invite_waiters: usize,
    pub bye_waiters: usize,
    pub response_waiters: usize,
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
