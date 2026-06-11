use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Event as IoEvent, IoEventType, Protocol, Zip};
use base::once_cell::sync::OnceCell;
use base::tokio::sync::mpsc::{self, Sender};
use base::tokio_util::sync::CancellationToken;

use crate::gb::SessionConf;
use crate::register::event::{self, Event};
pub(crate) use crate::register::io::{DeviceSession, Network};
use crate::register::schedule::TimeScheduler;
use crate::service::{stream_close, talk_close};
use crate::state::session::Cache as GeneralCache;

static REGISTER: OnceCell<Register> = OnceCell::new();

pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
pub const SERVER_HEART_SECOND: u64 = 60;
pub const SERVER_HEART_EXPIRE: Duration = Duration::from_secs(SERVER_HEART_SECOND);

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
    pub io_tx: Sender<Zip>,
    pub event_tx: Sender<Event>,
    pub io_map: Network,
}

impl Register {
    fn get() -> &'static Register {
        REGISTER.get().expect("Register not initialized")
    }

    pub fn init(
        server_conf: SessionConf,
        io_tx: Sender<Zip>,
        cancel_token: CancellationToken,
    ) -> GlobalResult<()> {
        if REGISTER.get().is_some() {
            return Ok(());
        }

        let (event_tx, event_rx) = mpsc::channel(256);
        TimeScheduler::init();
        let inner = Arc::new(Inner {
            server_conf,
            io_tx,
            event_tx,
            io_map: Network {
                session: Default::default(),
                net_device_map: Default::default(),
            },
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
        let inner = &Self::get().inner;
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
            let _ = Self::get()
                .inner
                .io_tx
                .try_send(Zip::build_event(IoEvent::new(
                    session.association.clone(),
                    IoEventType::Close,
                )));
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
    Duration::from_secs(u64::from(heartbeat_sec).saturating_mul(3))
}
