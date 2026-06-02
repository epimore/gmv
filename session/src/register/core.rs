use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Event as IoEvent, IoEventType, Protocol, Zip};
use base::once_cell::sync::OnceCell;
use base::tokio::sync::mpsc::{self, Sender};
use base::tokio_util::sync::CancellationToken;

use crate::gb::SessionConf;
use crate::gb::depot::SipPackage;
use crate::gb::handler::cmd::CmdStream;
use crate::register::event::{self, Event};
pub(crate) use crate::register::io::{DeviceSession, Network};
use crate::register::schedule::TimeScheduler;
use crate::register::session::Session;
use crate::state::session::Cache as GeneralCache;

static REGISTER: OnceCell<Register> = OnceCell::new();

pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
pub const SERVER_HEART_SECOND: u64 = 60;
pub const SERVER_HEART_EXPIRE: Duration = Duration::from_secs(SERVER_HEART_SECOND);

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    Device3Heart(Arc<str>),
    DeviceRegistration(Arc<str>),
    OutSession(u64),
    ServerHeart(Arc<str>),
}

pub struct Register {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub server_conf: SessionConf,
    pub io_tx: Sender<Zip>,
    pub sip_tx: Sender<SipPackage>,
    pub event_tx: Sender<Event>,
    pub io_map: Network,
    pub session_map: Session,
}

impl Register {
    fn get() -> &'static Register {
        REGISTER.get().expect("Register not initialized")
    }

    pub fn init(
        server_conf: SessionConf,
        io_tx: Sender<Zip>,
        sip_tx: Sender<SipPackage>,
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
            sip_tx,
            event_tx,
            io_map: Network {
                session: Default::default(),
                net_device_map: Default::default(),
            },
            session_map: Session {
                call_map: Default::default(),
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

        let Some(mut session) = arc.io_map.session.get_mut(device_id) else {
            return Err(GlobalError::new_sys_error(
                "unregistered device keepalive",
                |msg| warn!("device_id={device_id}; {msg}"),
            ));
        };

        if session.association == association {
            Self::scheduler()
                .refresh_register(&TimeScheduleKey::Device3Heart(device_id.clone()))?;
            return Ok(());
        }

        info!(
            "device {device_id} association changed: {} -> {}",
            session.association, association
        );

        arc.io_map
            .remove_device_mapping(device_id, &session.association);
        arc.io_map
            .update_net_device_mapping(&association, device_id);
        session.association = association;
        drop(session);

        Self::scheduler().refresh_register(&TimeScheduleKey::Device3Heart(device_id.clone()))?;
        Ok(())
    }

    pub fn register_device(device_id: Arc<str>, ds: DeviceSession) -> GlobalResult<()> {
        let arc = Self::get().inner.clone();

        let previous_session = Self::remove_device_by_inner(&device_id, &arc);
        let association_changed = previous_session
            .as_ref()
            .is_some_and(|previous_session| previous_session.association != ds.association);
        if let Some(previous_session) = previous_session {
            if association_changed {
                Self::close_tcp_if_needed(&previous_session);
            }
        }
        if association_changed {
            for stream in GeneralCache::reset_device_state(device_id.as_ref()) {
                let device_id = device_id.to_string();
                base::tokio::spawn(async move {
                    let _ = CmdStream::play_bye(
                        stream.seq,
                        stream.call_id,
                        &device_id,
                        &stream.channel_id,
                        &stream.from_tag,
                        &stream.to_tag,
                    )
                    .await;
                });
            }
        }

        let expires = Duration::from_secs((ds.heartbeat_sec * 3) as u64);
        Self::scheduler()
            .insert_register(TimeScheduleKey::Device3Heart(device_id.clone()), expires)
            .hand_log(|e| error!("insert device heartbeat timer failed: {e}"))?;

        Self::scheduler()
            .insert_register(
                TimeScheduleKey::DeviceRegistration(device_id.clone()),
                ds.registration_duration,
            )
            .hand_log(|e| error!("insert device registration timer failed: {e}"))?;

        arc.io_map.insert(device_id, ds);
        Ok(())
    }

    pub fn remove_device_by_inner(device_id: &Arc<str>, inner: &Inner) -> Option<DeviceSession> {
        let _ =
            Self::scheduler().remove_register(&TimeScheduleKey::Device3Heart(device_id.clone()));
        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::DeviceRegistration(device_id.clone()));

        if let Some((_, session)) = inner.io_map.session.remove(device_id) {
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
    }

    pub fn remove_device_by_association(association: &Association) -> Option<DeviceSession> {
        let inner = &Self::get().inner;
        let (_, device_id) = inner.io_map.net_device_map.remove(association)?;
        let _ =
            Self::scheduler().remove_register(&TimeScheduleKey::Device3Heart(device_id.clone()));
        let _ = Self::scheduler()
            .remove_register(&TimeScheduleKey::DeviceRegistration(device_id.clone()));
        inner
            .io_map
            .session
            .remove(&device_id)
            .map(|(_, session)| session)
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
            .map(|item| DeviceSession {
                contact_uri: item.contact_uri.clone(),
                association: item.association.clone(),
                association_expire: AtomicBool::new(
                    item.association_expire.load(Ordering::Relaxed),
                ),
                support_lr: AtomicBool::new(item.support_lr.load(Ordering::Relaxed)),
                heartbeat_sec: item.heartbeat_sec,
                last_seen: item.last_seen,
                registration_duration: item.registration_duration,
            })
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
