use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base::chrono::{Duration as TimeDelta, Local};
use base::dashmap::DashMap;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Protocol};
use base::once_cell::sync::OnceCell;
use base::tokio::sync::Semaphore;
use base::tokio::sync::mpsc::{self, Sender};
use base::tokio_util::sync::CancellationToken;

use crate::gb::SessionConf;
use crate::gb::sip::NativeSipRuntimeHandle;
use crate::register::event::{self, Event};
pub(crate) use crate::register::network::{DeviceSession, Network};
use crate::register::schedule::TimeScheduler;
use crate::service::{stream_close, talk_close};
use crate::state::session::Cache as GeneralCache;
use crate::storage::db_task::{self, DbTask};
use crate::storage::entity::{GmvDevice, GmvOauth};

static REGISTER: OnceCell<Register> = OnceCell::new();

pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
pub const SERVER_HEART_SECOND: u64 = 60;
pub const SERVER_HEART_EXPIRE: Duration = Duration::from_secs(SERVER_HEART_SECOND);
const MAX_DEVICE_RECOVERY_CONCURRENCY: usize = 64;

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    Device3Heart(Arc<str>),
    DeviceRegistration(Arc<str>),
    DeviceReconnect(Arc<str>, u64),
    StreamClosing(Arc<str>, u64),
    TalkClosing(Arc<str>, u64),
    CatalogSubscription(Arc<str>, u64),
    OutSession(u64),
    ServerHeart(Arc<str>),
}

pub struct Register {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub server_conf: SessionConf,
    pub event_tx: Sender<Event>,
    pub io_map: Network,
    recovering_devices: DashMap<Arc<str>, ()>,
    device_recovery_limit: Arc<Semaphore>,
}

impl Register {
    fn get() -> &'static Register {
        REGISTER.get().expect("Register not initialized")
    }

    pub fn init(server_conf: SessionConf, cancel_token: CancellationToken) -> GlobalResult<()> {
        if REGISTER.get().is_some() {
            return Ok(());
        }

        let (event_tx, event_rx) = mpsc::channel(256);
        TimeScheduler::init();
        let inner = Arc::new(Inner {
            server_conf,
            event_tx,
            io_map: Network {
                session: Default::default(),
                net_device_map: Default::default(),
            },
            recovering_devices: DashMap::new(),
            device_recovery_limit: Arc::new(Semaphore::new(MAX_DEVICE_RECOVERY_CONCURRENCY)),
        });

        REGISTER
            .set(Register {
                inner: inner.clone(),
            })
            .map_err(|_| GlobalError::new_sys_error("Register already initialized", |_| {}))?;

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
        if inner
            .recovering_devices
            .insert(device_id.clone(), ())
            .is_some()
        {
            return Ok(());
        }
        let permit = match inner.device_recovery_limit.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                inner.recovering_devices.remove(&device_id);
                warn!(
                    "device keepalive recovery concurrency is full; retry on next keepalive: \
                     device_id={device_id}"
                );
                return Ok(());
            }
        };
        base::tokio::spawn(async move {
            let _permit = permit;
            let result = Self::recover_device_session(&device_id, association).await;
            inner.recovering_devices.remove(&device_id);
            match result {
                Ok(()) => db_task::submit(DbTask::TouchDeviceHeartbeat {
                    device_id: device_id.to_string(),
                }),
                Err(err) => warn!(
                    "recover device session from keepalive failed: device_id={device_id}, err={err}"
                ),
            }
        });
        Ok(())
    }

    async fn recover_device_session(
        device_id: &Arc<str>,
        association: Association,
    ) -> GlobalResult<()> {
        if Self::has_session(device_id) {
            return Self::device_heart(device_id, association);
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
            oauth.heartbeat_sec,
            Duration::from_secs(remaining),
        );
        session.set_gb_version(device.gb_version);
        if device.enable_lr != 0 {
            session.enable_lr();
        }
        Self::register_device(device_id.clone(), session)
    }

    pub fn register_device(device_id: Arc<str>, ds: DeviceSession) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();

        let heartbeat_sec = ds.heartbeat_sec;
        let registration_duration = ds.registration_duration;
        let new_generation = arc
            .io_map
            .session
            .get(&device_id)
            .is_some_and(|current| current.registration_generation_changed(&ds));
        let ds = if new_generation {
            ds
        } else {
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
                    stream_close::retry_device(device_id.as_ref());
                    talk_close::retry_device(device_id.as_ref());
                    return Ok(());
                }
                Err(ds) => ds,
            }
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
        if new_generation {
            warn!(
                "new registration generation, cleanup old device state: device_id={}",
                device_id
            );
            GeneralCache::reset_device_state(device_id.as_ref());
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
        if !new_generation {
            stream_close::retry_device(device_id.as_ref());
            talk_close::retry_device(device_id.as_ref());
        }
        Ok(())
    }

    pub fn remove_device_by_inner(device_id: &Arc<str>, inner: &Inner) -> Option<DeviceSession> {
        let _ =
            Self::scheduler().remove_register(&TimeScheduleKey::Device3Heart(device_id.clone()));
        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::DeviceRegistration(device_id.clone()));

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
        Self::remove_device_by_inner(device_id, inner);
        GeneralCache::reset_device_state(device_id.as_ref());
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
        GeneralCache::reset_device_state(device_id.as_ref());
        let _ = inner
            .event_tx
            .try_send(Event::DeviceOffline(device_id.clone()))
            .hand_log(|msg| error!("{msg}"));
        Some(session)
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

    pub fn get_connected_device_session(device_id: &str) -> Option<DeviceSession> {
        Self::get().inner.io_map.connected_session(device_id)
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

    pub async fn server_keep_heart_update_db(domain_id: Arc<str>) -> GlobalResult<()> {
        let update_res = Self::get().inner.server_conf.heart_to_db().await;
        Self::scheduler()
            .insert_register(TimeScheduleKey::ServerHeart(domain_id), SERVER_HEART_EXPIRE)?;
        update_res
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

#[cfg(test)]
mod tests {
    use super::heartbeat_timeout;
    use std::time::Duration;

    #[test]
    fn heartbeat_timeout_includes_one_second_grace() {
        assert_eq!(heartbeat_timeout(60), Duration::from_secs(181));
    }
}
