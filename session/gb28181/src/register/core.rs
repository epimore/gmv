use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base::chrono::{Duration as TimeDelta, Local};
use base::dashmap::DashMap;
use base::dashmap::mapref::entry::Entry;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::net::state::{Association, Protocol};
use base::once_cell::sync::OnceCell;
use base::tokio::sync::Semaphore;
use base::tokio::sync::mpsc::{self, Sender};
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::{SipRegisteredSource, SipTransportProtocol};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::gb::sip::NativeSipRuntimeHandle;
use crate::register::event::{self, Event};
pub(crate) use crate::register::network::{DeviceSession, Network, RegistrationClass};
use crate::register::schedule::TimeScheduler;
use crate::service::{stream_close, talk_close};
use crate::state::session::Cache as GeneralCache;
use crate::storage::db_task::{self, DbTask};
use crate::storage::entity::{GmvDevice, GmvOauth};

static REGISTER: OnceCell<Register> = OnceCell::new();

pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
const MAX_DEVICE_RECOVERY_CONCURRENCY: usize = 64;

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    Device3Heart(Arc<str>),
    DeviceRegistration(Arc<str>),
    DeviceReconnect(Arc<str>, u64),
    StreamClosing(Arc<str>, u64),
    TalkClosing(Arc<str>, u64),
    CatalogSubscription(Arc<str>, u64),
    PlaybackPauseExpiry(Arc<str>, u64),
    PlaybackPresenceExpiry(Arc<str>, u64),
    OutSession(u64),
}

pub struct Register {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub event_tx: Sender<Event>,
    pub io_map: Network,
    recovering_devices: DashMap<Arc<str>, u64>,
    next_recovery_id: AtomicU64,
    device_transition_lock: Mutex<()>,
    device_recovery_limit: Arc<Semaphore>,
    device_recovery_failure_episode: Mutex<FailureEpisode>,
}

enum DeviceRecoveryOutcome {
    Recovered { registration_expires: u32 },
    AlreadyLive,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterOutcome {
    NewEpoch,
    RecoveredEpoch,
    Refresh,
    Retransmission,
    BindingReplacement,
    Rebind,
    Stale,
}

impl RegisterOutcome {
    pub fn needs_post_online_sync(self) -> bool {
        matches!(self, Self::NewEpoch | Self::RecoveredEpoch)
    }
}

impl Register {
    fn get() -> &'static Register {
        REGISTER.get().expect("Register not initialized")
    }

    pub fn init(cancel_token: CancellationToken) -> GlobalResult<()> {
        if REGISTER.get().is_some() {
            return Ok(());
        }

        let (event_tx, event_rx) = mpsc::channel(256);
        TimeScheduler::init();
        let inner = Arc::new(Inner {
            event_tx,
            io_map: Network {
                session: Default::default(),
                net_device_map: Default::default(),
            },
            recovering_devices: DashMap::new(),
            next_recovery_id: AtomicU64::new(1),
            device_transition_lock: Mutex::new(()),
            device_recovery_limit: Arc::new(Semaphore::new(MAX_DEVICE_RECOVERY_CONCURRENCY)),
            device_recovery_failure_episode: Mutex::new(FailureEpisode::default()),
        });

        REGISTER
            .set(Register {
                inner: inner.clone(),
            })
            .map_err(|_| {
                GlobalError::new_sys_error("Register already initialized", |msg| error!("{msg}"))
            })?;

        base::tokio::spawn(event::schedule_event(inner, event_rx, cancel_token));
        Ok(())
    }

    pub fn scheduler() -> &'static TimeScheduler {
        TimeScheduler::global()
    }

    pub fn active_device_count() -> usize {
        Self::get().inner.io_map.session.len()
    }

    pub fn device_heart(device_id: &Arc<str>, association: Association) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();

        let Some(previous_session) = arc
            .io_map
            .session
            .get(device_id)
            .map(|item| item.snapshot())
        else {
            return Err(GlobalError::new_sys_error(
                "unregistered device keepalive",
                |msg| warn!("device_id={device_id}; {msg}"),
            ));
        };

        if previous_session.association != association {
            info!(
                "device {device_id} association changed: {} -> {}",
                previous_session.association, association
            );
        }
        let Some(rebind_result) = arc.io_map.rebind(device_id, association.clone()) else {
            return Err(GlobalError::new_sys_error(
                "device session disappeared during keepalive",
                |msg| warn!("device_id={device_id}; {msg}"),
            ));
        };
        if previous_session.association != association {
            Self::close_tcp_if_needed(&previous_session);
        }

        let reconnected = rebind_result.reconnect_generation.is_some();
        if let Some(generation) = rebind_result.reconnect_generation {
            let _ = Self::scheduler().remove_register(&TimeScheduleKey::DeviceReconnect(
                device_id.clone(),
                generation,
            ));
            Self::scheduler().insert_register(
                TimeScheduleKey::Device3Heart(device_id.clone()),
                heartbeat_timeout(previous_session.heartbeat_sec),
            )?;
        } else {
            Self::scheduler()
                .refresh_register(&TimeScheduleKey::Device3Heart(device_id.clone()))?;
        }
        if reconnected || previous_session.association != association {
            stream_close::retry_device(device_id);
            talk_close::retry_device(device_id);
        }
        Ok(())
    }

    pub fn recover_device_on_keepalive(
        device_id: Arc<str>,
        association: Association,
    ) -> GlobalResult<()> {
        if Self::has_session(&device_id) {
            Self::device_heart(&device_id, association)?;
            db_task::submit(DbTask::TouchDeviceHeartbeat {
                device_id: device_id.to_string(),
            });
            return Ok(());
        }

        let inner = Self::get().inner.clone();
        let recovery_id = inner.next_recovery_id.fetch_add(1, Ordering::Relaxed);
        match inner.recovering_devices.entry(device_id.clone()) {
            Entry::Occupied(_) => return Ok(()),
            Entry::Vacant(entry) => {
                entry.insert(recovery_id);
            }
        }
        let permit = match inner.device_recovery_limit.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                inner.recovering_devices.remove(&device_id);
                base::log::trace!(
                    "device keepalive recovery attempt deferred: device_id={device_id}, reason=concurrency_full"
                );
                record_device_recovery_failure(&inner, "concurrency_full");
                return Ok(());
            }
        };
        base::tokio::spawn(async move {
            let _permit = permit;
            let result = Self::recover_device_session(&device_id, association, recovery_id).await;
            inner
                .recovering_devices
                .remove_if(&device_id, |_, current| *current == recovery_id);
            match result {
                Ok(DeviceRecoveryOutcome::Recovered {
                    registration_expires,
                }) => {
                    record_device_recovery_success(&inner);
                    db_task::submit(DbTask::TouchDeviceHeartbeat {
                        device_id: device_id.to_string(),
                    });
                    crate::gb::sip::adapter::schedule_post_online_sync(
                        device_id.to_string(),
                        registration_expires,
                    );
                }
                Ok(DeviceRecoveryOutcome::AlreadyLive) => {
                    record_device_recovery_success(&inner);
                    db_task::submit(DbTask::TouchDeviceHeartbeat {
                        device_id: device_id.to_string(),
                    });
                }
                Ok(DeviceRecoveryOutcome::Superseded) => {
                    base::log::trace!(
                        "device keepalive recovery superseded: device_id={device_id}"
                    );
                }
                Err(err) => {
                    base::log::trace!(
                        "recover device session from keepalive failed: device_id={device_id}, err={err}"
                    );
                    record_device_recovery_failure(&inner, "recovery_failed");
                }
            }
        });
        Ok(())
    }

    async fn recover_device_session(
        device_id: &Arc<str>,
        association: Association,
        recovery_id: u64,
    ) -> GlobalResult<DeviceRecoveryOutcome> {
        if Self::has_session(device_id) {
            Self::device_heart(device_id, association)?;
            return Ok(DeviceRecoveryOutcome::AlreadyLive);
        }
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id)
            .await?
            .ok_or_else(|| {
                invalid_device_lease(device_id, "enabled device authorization is missing")
            })?;
        let device_id_string = device_id.to_string();
        let device = GmvDevice::query_gmv_device_by_device_id(&device_id_string)
            .await?
            .ok_or_else(|| {
                invalid_device_lease(device_id, "device registration snapshot is missing")
            })?;

        let expected_protocol = if device.transport.eq_ignore_ascii_case("UDP") {
            Protocol::UDP
        } else if device.transport.eq_ignore_ascii_case("TCP") {
            Protocol::TCP
        } else {
            return Err(invalid_device_lease(
                device_id,
                "device registration transport is unsupported",
            ));
        };
        if association.protocol != expected_protocol {
            return Err(invalid_device_lease(
                device_id,
                "keepalive transport does not match device registration",
            ));
        }

        let stored_remote = device
            .local_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|_| invalid_device_lease(device_id, "stored device address is invalid"))?;
        if stored_remote.ip() != association.remote_addr.ip() {
            return Err(invalid_device_lease(
                device_id,
                "keepalive source IP does not match device registration",
            ));
        }

        let now = Local::now().naive_local();
        let registration_expires_at =
            device.register_time + TimeDelta::seconds(i64::from(device.register_expires));
        let online_expires_at = device
            .online_expire_time
            .ok_or_else(|| invalid_device_lease(device_id, "device online expiry is missing"))?;
        if registration_expires_at <= now || online_expires_at <= now {
            return Err(invalid_device_lease(
                device_id,
                "device registration or online lease has expired",
            ));
        }
        let remaining = registration_expires_at
            .signed_duration_since(now)
            .num_seconds()
            .max(1) as u64;
        let mut session = DeviceSession::build(
            device.contact_uri,
            association,
            oauth.heartbeat_sec_u8()?,
            Duration::from_secs(remaining),
        );
        if device.registration_epoch_closed_at.is_some() {
            return Err(invalid_device_lease(
                device_id,
                "device registration epoch is closed",
            ));
        }
        session.set_optional_registration_epoch_id(device.registration_epoch_id);
        session.mark_registration_snapshot_restored();
        session.set_registration_identity(
            device.registration_call_id.unwrap_or_default(),
            device
                .registration_cseq
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or_default(),
        );
        session.set_gb_version(device.gb_version);
        session.mark_registration_ready();
        if device.enable_lr != 0 {
            session.enable_lr();
        }
        if Self::register_recovered_device(device_id.clone(), session, recovery_id)? {
            Ok(DeviceRecoveryOutcome::Recovered {
                registration_expires: u32::try_from(remaining).unwrap_or(u32::MAX),
            })
        } else {
            Ok(DeviceRecoveryOutcome::Superseded)
        }
    }

    pub fn register_device(
        device_id: Arc<str>,
        ds: DeviceSession,
    ) -> GlobalResult<RegisterOutcome> {
        let arc = Self::get().inner.clone();
        arc.recovering_devices.remove(&device_id);
        let _transition = arc.device_transition_lock.lock();
        Self::register_device_by_inner(device_id, ds, &arc)
    }

    fn register_recovered_device(
        device_id: Arc<str>,
        ds: DeviceSession,
        recovery_id: u64,
    ) -> GlobalResult<bool> {
        let arc = Self::get().inner.clone();
        let _transition = arc.device_transition_lock.lock();
        let recovery_is_current = arc
            .recovering_devices
            .get(&device_id)
            .is_some_and(|current| *current == recovery_id);
        if !recovery_is_current || arc.io_map.session.contains_key(&device_id) {
            return Ok(false);
        }
        Self::register_device_by_inner(device_id, ds, &arc)?;
        Ok(true)
    }

    fn register_device_by_inner(
        device_id: Arc<str>,
        mut ds: DeviceSession,
        arc: &Arc<Inner>,
    ) -> GlobalResult<RegisterOutcome> {
        let heartbeat_sec = ds.heartbeat_sec;
        let registration_duration = ds.registration_duration;
        let association = ds.association.clone();
        let registration_call_id = ds.registration_call_id.clone();
        let registration_cseq = ds.registration_cseq;
        let classification = arc
            .io_map
            .session
            .get(&device_id)
            .map(|current| current.classify_registration(&ds));
        if matches!(classification, Some(RegistrationClass::Stale)) {
            warn!(
                "stale REGISTER ignored: device_id={}, call_id={}, cseq={}",
                device_id, ds.registration_call_id, ds.registration_cseq
            );
            return Ok(RegisterOutcome::Stale);
        }
        if matches!(classification, Some(RegistrationClass::Retransmission)) {
            return Ok(RegisterOutcome::Retransmission);
        }
        let outcome = if let Some(current) = arc.io_map.session.get(&device_id) {
            ds.registration_epoch_id = current.registration_epoch_id.clone();
            if current.registration_is_ready() {
                ds.mark_registration_ready();
            }
            match classification.unwrap_or(RegistrationClass::Refresh) {
                RegistrationClass::Refresh => RegisterOutcome::Refresh,
                RegistrationClass::BindingReplacement => RegisterOutcome::BindingReplacement,
                RegistrationClass::Rebind => RegisterOutcome::Rebind,
                RegistrationClass::Retransmission => RegisterOutcome::Retransmission,
                RegistrationClass::Stale => RegisterOutcome::Stale,
            }
        } else if ds.registration_snapshot_restored {
            RegisterOutcome::RecoveredEpoch
        } else {
            ds.set_registration_epoch_id(Uuid::new_v4().to_string());
            RegisterOutcome::NewEpoch
        };
        let ds = if matches!(outcome, RegisterOutcome::Rebind) {
            match arc.io_map.rebind_registration(&device_id, ds) {
                Ok(generation) => {
                    let _ = Self::scheduler().remove_register(&TimeScheduleKey::DeviceReconnect(
                        device_id.clone(),
                        generation,
                    ));
                    Self::scheduler()
                        .insert_register(
                            TimeScheduleKey::Device3Heart(device_id.clone()),
                            heartbeat_timeout(heartbeat_sec),
                        )
                        .hand_log(|e| error!("insert device heartbeat timer failed: {e}"))?;
                    Self::scheduler()
                        .insert_register(
                            TimeScheduleKey::DeviceRegistration(device_id.clone()),
                            registration_duration,
                        )
                        .hand_log(|e| error!("insert device registration timer failed: {e}"))?;
                    sync_native_registered_source(
                        device_id.as_ref(),
                        &association,
                        &registration_call_id,
                        registration_cseq,
                    );
                    stream_close::retry_device(device_id.as_ref());
                    talk_close::retry_device(device_id.as_ref());
                    return Ok(outcome);
                }
                Err(ds) => ds,
            }
        } else {
            ds
        };

        let previous_session = Self::remove_device_by_inner(&device_id, &arc);
        let association_changed = previous_session
            .as_ref()
            .is_some_and(|previous_session| previous_session.association != ds.association);
        if let Some(previous_session) = previous_session {
            if association_changed {
                Self::close_tcp_if_needed(&previous_session);
            }
        }
        let expires = heartbeat_timeout(ds.heartbeat_sec);
        Self::scheduler()
            .insert_register(TimeScheduleKey::Device3Heart(device_id.clone()), expires)
            .hand_log(|e| error!("insert device heartbeat timer failed: {e}"))?;

        Self::scheduler()
            .insert_register(
                TimeScheduleKey::DeviceRegistration(device_id.clone()),
                ds.registration_duration,
            )
            .hand_log(|e| error!("insert device registration timer failed: {e}"))?;

        arc.io_map.insert(device_id.clone(), ds);
        sync_native_registered_source(
            device_id.as_ref(),
            &association,
            &registration_call_id,
            registration_cseq,
        );
        if !matches!(outcome, RegisterOutcome::NewEpoch) {
            stream_close::retry_device(device_id.as_ref());
            talk_close::retry_device(device_id.as_ref());
        }
        Ok(outcome)
    }

    pub fn remove_device_by_inner(device_id: &Arc<str>, inner: &Inner) -> Option<DeviceSession> {
        let _ =
            Self::scheduler().remove_register(&TimeScheduleKey::Device3Heart(device_id.clone()));
        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::DeviceRegistration(device_id.clone()));
        remove_native_registered_source(device_id.as_ref());

        if let Some((_, session)) = inner.io_map.session.remove(device_id) {
            let generation = session.connection_generation.load(Ordering::Acquire);
            let _ = Self::scheduler().remove_register(&TimeScheduleKey::DeviceReconnect(
                device_id.clone(),
                generation,
            ));
            if !session.association_expire.load(Ordering::Relaxed) {
                inner.io_map.net_device_map.remove(&session.association);
            }
            return Some(session);
        }
        None
    }

    pub fn remove_device(device_id: &Arc<str>) {
        let inner = &Self::get().inner;
        inner.recovering_devices.remove(device_id);
        let _transition = inner.device_transition_lock.lock();
        let registration_epoch_id = Self::remove_device_by_inner(device_id, inner)
            .and_then(|session| session.registration_epoch_id);
        crate::service::dialog_epoch::close(
            device_id.as_ref(),
            registration_epoch_id,
            "explicit_unregister",
        );
    }

    pub fn detach_device_association(association: &Association) -> bool {
        let Some(register) = REGISTER.get() else {
            return false;
        };
        let inner = &register.inner;
        let Some(detached) = inner.io_map.detach_association(association) else {
            return false;
        };

        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::Device3Heart(detached.device_id.clone()));
        let key = TimeScheduleKey::DeviceReconnect(detached.device_id.clone(), detached.generation);
        if let Err(err) = Self::scheduler().insert_register(key, detached.timeout) {
            error!(
                "schedule device reconnect cleanup failed: device_id={}, generation={}, err={err}",
                detached.device_id, detached.generation
            );
            Self::expire_disconnected_by_inner(&detached.device_id, detached.generation, inner);
        }
        true
    }

    pub fn expire_disconnected_by_inner(
        device_id: &Arc<str>,
        generation: u64,
        inner: &Inner,
    ) -> Option<DeviceSession> {
        let session = inner.io_map.remove_disconnected(device_id, generation)?;
        let _ =
            Self::scheduler().remove_register(&TimeScheduleKey::Device3Heart(device_id.clone()));
        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::DeviceRegistration(device_id.clone()));
        let _ = Self::scheduler().remove_register(&TimeScheduleKey::DeviceReconnect(
            device_id.clone(),
            generation,
        ));
        crate::service::dialog_epoch::close(
            device_id.as_ref(),
            session.registration_epoch_id.clone(),
            "reconnect_timeout",
        );
        Some(session)
    }

    pub(crate) fn close_removed_session(
        device_id: &Arc<str>,
        session: &DeviceSession,
        reason: &'static str,
    ) {
        crate::service::dialog_epoch::close(
            device_id.as_ref(),
            session.registration_epoch_id.clone(),
            reason,
        );
    }

    pub fn get_device_id_by_association(association: &Association) -> Option<Arc<str>> {
        Self::get()
            .inner
            .io_map
            .net_device_map
            .get(association)
            .map(|item| item.clone())
    }

    pub fn get_device_session(device_id: &str) -> Option<DeviceSession> {
        Self::get()
            .inner
            .io_map
            .session
            .get(device_id)
            .map(|item| item.snapshot())
    }

    pub fn registration_epoch_id(device_id: &str) -> Option<String> {
        Self::get_device_session(device_id).and_then(|session| session.registration_epoch_id)
    }

    pub fn registration_epoch_matches(
        device_id: &str,
        expected_registration_epoch_id: Option<&str>,
    ) -> bool {
        Self::get_device_session(device_id).is_some_and(|session| {
            session.registration_epoch_id.as_deref() == expected_registration_epoch_id
        })
    }

    pub fn mark_registration_ready(
        device_id: &str,
        expected_registration_epoch_id: Option<&str>,
    ) -> bool {
        Self::get()
            .inner
            .io_map
            .session
            .get(device_id)
            .is_some_and(|session| {
                if session.registration_epoch_id.as_deref() != expected_registration_epoch_id {
                    return false;
                }
                session.mark_registration_ready();
                true
            })
    }

    pub fn get_connected_device_session(device_id: &str) -> Option<DeviceSession> {
        Self::get().inner.io_map.connected_session(device_id)
    }

    pub fn validate_registered_source(
        device_id: &str,
        association: &Association,
    ) -> GlobalResult<()> {
        let session = Self::get_connected_device_session(device_id).ok_or_else(|| {
            invalid_device_lease(
                device_id,
                "device is not connected for SIP business message",
            )
        })?;
        if session.association.protocol != association.protocol {
            return Err(invalid_device_lease(
                device_id,
                "SIP business message transport does not match registration",
            ));
        }
        if matches!(association.protocol, Protocol::UDP) {
            if session.association.remote_addr.ip() == association.remote_addr.ip() {
                return Ok(());
            }
            return Err(invalid_device_lease(
                device_id,
                "SIP business message source IP does not match registration",
            ));
        }
        if session.association == *association {
            return Ok(());
        }
        Err(invalid_device_lease(
            device_id,
            "SIP business message association does not match registration",
        ))
    }

    pub fn has_session(device_id: &str) -> bool {
        Self::get().inner.io_map.session.contains_key(device_id)
    }

    pub fn close_tcp_if_needed(session: &DeviceSession) {
        if matches!(session.association.protocol, Protocol::TCP) {
            if let Ok(runtime) = NativeSipRuntimeHandle::global() {
                runtime.close_transport(&session.association, 1);
            }
        }
    }
}

fn record_device_recovery_failure(inner: &Inner, reason: &str) {
    match inner
        .device_recovery_failure_episode
        .lock()
        .record_failure(Instant::now())
    {
        EpisodeDecision::Started => warn!(
            "device keepalive recovery state changed: state=degraded, previous_state=ready, reason={reason}"
        ),
        EpisodeDecision::Summary {
            total,
            since_last_summary,
            suppressed,
            duration,
        } => warn!(
            "device keepalive recovery remains degraded: state=degraded, outcome=ongoing, latest_reason={reason}, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
            duration.as_millis()
        ),
        EpisodeDecision::Suppressed => {}
        EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
    }
}

fn record_device_recovery_success(inner: &Inner) {
    if let EpisodeDecision::Recovered {
        total,
        suppressed,
        duration,
    } = inner
        .device_recovery_failure_episode
        .lock()
        .record_success(Instant::now())
    {
        info!(
            "device keepalive recovery state changed: state=ready, previous_state=degraded, outcome=recovered, total_failures={total}, suppressed={suppressed}, duration_ms={}",
            duration.as_millis()
        );
    }
}

fn heartbeat_timeout(heartbeat_sec: u8) -> Duration {
    Duration::from_secs(u64::from(heartbeat_sec).saturating_mul(3).saturating_add(1))
}

fn invalid_device_lease(device_id: &str, message: &str) -> GlobalError {
    GlobalError::new_sys_error(message, |log_message| {
        warn!("device_id={device_id}; {log_message}")
    })
}

fn sync_native_registered_source(
    device_id: &str,
    association: &Association,
    registration_call_id: &str,
    registration_cseq: u32,
) {
    let protocol = match association.protocol {
        Protocol::UDP => SipTransportProtocol::Udp,
        Protocol::TCP => SipTransportProtocol::Tcp,
        Protocol::ALL => return,
    };
    let Ok(runtime) = NativeSipRuntimeHandle::global() else {
        return;
    };
    if let Err(err) = runtime.allow_registered_source(SipRegisteredSource {
        device_id: device_id.to_string(),
        remote_address: association.remote_addr.ip().to_string(),
        protocol,
        registration_call_id: (!registration_call_id.is_empty())
            .then(|| registration_call_id.to_string()),
        registration_cseq: (registration_cseq > 0).then_some(registration_cseq),
    }) {
        warn!("sync native SIP registered source failed: device_id={device_id}, err={err}");
    }
}

fn remove_native_registered_source(device_id: &str) {
    let Ok(runtime) = NativeSipRuntimeHandle::global() else {
        return;
    };
    if let Err(err) = runtime.remove_registered_source(device_id.to_string()) {
        warn!("remove native SIP registered source failed: device_id={device_id}, err={err}");
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceRecoveryOutcome, Register, heartbeat_timeout};
    use crate::storage::entity::{
        GmvDevice, GmvOauth, TEST_STORAGE_TEST_LOCK, enable_test_storage,
    };
    use base::chrono::{Duration as TimeDelta, Local};
    use base::net::state::{Association, Protocol};
    use base::tokio::runtime::Builder;
    use base::tokio_util::sync::CancellationToken;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn heartbeat_timeout_includes_one_second_grace() {
        assert_eq!(heartbeat_timeout(60), Duration::from_secs(181));
    }

    #[test]
    fn valid_keepalive_restores_device_session_from_persisted_lease() {
        let _test_lock = TEST_STORAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _storage = enable_test_storage(GmvOauth {
            device_id: "34020000001320000001".to_string(),
            domain_id: "34020000002000000001".to_string(),
            domain: "3402000000".to_string(),
            status: 1,
            heartbeat_sec: 60,
            ..GmvOauth::default()
        });
        let cancel = CancellationToken::new();

        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                Register::init(cancel.child_token()).expect("register init");
                let now = Local::now().naive_local();
                let device_id: Arc<str> = Arc::from("34020000001320000001");
                GmvDevice {
                    device_id: device_id.to_string(),
                    transport: "UDP".to_string(),
                    register_expires: 300,
                    register_time: now - TimeDelta::seconds(10),
                    online_expire_time: Some(now + TimeDelta::seconds(120)),
                    local_addr: "192.0.2.10:15060".to_string(),
                    contact_uri: "sip:34020000001320000001@192.0.2.10:15060".to_string(),
                    ..GmvDevice::default()
                }
                .insert_single_gmv_device_by_register()
                .await
                .expect("persist device snapshot");
                let recovery_id = 7;
                Register::get()
                    .inner
                    .recovering_devices
                    .insert(device_id.clone(), recovery_id);
                let association = Association::new(
                    "127.0.0.1:5060"
                        .parse::<SocketAddr>()
                        .expect("local address"),
                    "192.0.2.10:16060"
                        .parse::<SocketAddr>()
                        .expect("remote address"),
                    Protocol::UDP,
                );

                let outcome =
                    Register::recover_device_session(&device_id, association.clone(), recovery_id)
                        .await
                        .expect("recover device session");
                assert!(matches!(outcome, DeviceRecoveryOutcome::Recovered { .. }));
                let session = Register::get_device_session(&device_id).expect("device session");
                assert_eq!(session.association, association);
                Register::remove_device(&device_id);
                cancel.cancel();
            });
    }
}
