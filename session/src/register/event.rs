use std::sync::Arc;

use base::exception::GlobalResultExt;
use base::log::{debug, error, warn};
use base::tokio;
use base::tokio::select;
use base::tokio::sync::Semaphore;
use base::tokio::sync::mpsc::Receiver;
use base::tokio_util::sync::CancellationToken;

use crate::gb::depot::trans::TransactionContext;
use crate::gb::handler::cmd::CmdQuery;
use crate::register::core::{Inner, Register, TimeScheduleKey};
use crate::register::schedule::ScheduleKey;
use crate::state::session::Cache as GeneralCache;
use crate::storage::db_task::{self, DbTask};

const MAX_WORKER_POOL: usize = 128;

#[derive(Clone, Eq, PartialEq)]
pub enum Event {
    DeviceOffline(Arc<str>),
    ServerHeart(Arc<str>),
    RefreshCatalogSubscription(Arc<str>, u64),
    OutSession(u64),
}

pub async fn schedule_event(
    inner: Arc<Inner>,
    mut event_rx: Receiver<Event>,
    cancel_token: CancellationToken,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_WORKER_POOL));
    loop {
        select! {
            biased;
            batch = Register::scheduler().next_batch(&cancel_token) => {
                match batch {
                    Some(items) => on_time_schedule(&inner, items).await,
                    None => break,
                }
            }
            _ = handle_rx_event(&mut event_rx, semaphore.clone()) => {}
            _ = cancel_token.cancelled() => break,
        }
    }
}

async fn handle_rx_event(rx: &mut Receiver<Event>, semaphore: Arc<Semaphore>) {
    if let Some(event) = rx.recv().await {
        if let Ok(permit) = semaphore
            .acquire_owned()
            .await
            .hand_log(|msg| error!("{msg}"))
        {
            tokio::spawn(async move {
                let _permit = permit;
                hand_event(event).await;
            });
        }
    }
}

async fn hand_event(event: Event) {
    match event {
        Event::DeviceOffline(device_id) => {
            db_task::submit(DbTask::ExpireDeviceOnline {
                device_id: device_id.to_string(),
            });
        }
        Event::ServerHeart(domain_id) => {
            let _ = Register::server_keep_heart_update_db(domain_id).await;
        }
        Event::RefreshCatalogSubscription(device_id, generation) => {
            let _ = CmdQuery::refresh_catalog_subscription(device_id, generation)
                .await
                .hand_log(|msg| {
                    warn!("refresh catalog subscription failed: {msg}")
                });
        }
        Event::OutSession(_) => {}
    }
}

async fn on_time_schedule(
    inner: &Inner,
    batch: Vec<crate::register::schedule::ScheduleEvent<ScheduleKey>>,
) {
    let mut trans_keys = Vec::new();
    let mut cache_keys = Vec::new();

    for event in batch {
        match event.key {
            ScheduleKey::Register(TimeScheduleKey::Device3Heart(device_id))
            | ScheduleKey::Register(TimeScheduleKey::DeviceRegistration(device_id)) => {
                warn!("device {} expired, removing session", device_id);
                if let Some(session) = Register::remove_device_by_inner(&device_id, inner) {
                    GeneralCache::reset_device_state(device_id.as_ref());
                    Register::close_tcp_if_needed(&session);
                    let _ = inner
                        .event_tx
                        .try_send(Event::DeviceOffline(device_id))
                        .hand_log(|msg| error!("{msg}"));
                }
            }
            ScheduleKey::Register(TimeScheduleKey::DeviceReconnect(device_id, generation)) => {
                if Register::expire_disconnected_by_inner(&device_id, generation, inner).is_some() {
                    warn!(
                        "device reconnect expired, session removed: device_id={}, generation={}",
                        device_id, generation
                    );
                } else {
                    debug!(
                        "ignore stale device reconnect event: device_id={}, generation={}",
                        device_id, generation
                    );
                }
            }
            ScheduleKey::Register(TimeScheduleKey::StreamClosing(stream_id, generation)) => {
                if let Some(info) =
                    GeneralCache::stream_close_force(stream_id.as_ref(), generation)
                {
                    warn!(
                        "force cleanup closing stream: device_id={}, channel_id={}, \
                         stream_id={}, ssrc={}, call_id={}, generation={}, reason={}",
                        info.device_id,
                        info.channel_id,
                        info.stream_id,
                        info.ssrc,
                        info.call_id,
                        info.generation,
                        info.last_error.as_deref().unwrap_or("close deadline expired")
                    );
                } else {
                    debug!(
                        "ignore stale stream closing event: stream_id={}, generation={}",
                        stream_id, generation
                    );
                }
            }
            ScheduleKey::Register(TimeScheduleKey::TalkClosing(talk_id, generation)) => {
                if let Some(info) =
                    GeneralCache::talk_close_force(talk_id.as_ref(), generation)
                {
                    warn!(
                        "force cleanup closing talk: device_id={}, channel_id={}, \
                         talk_id={}, ssrc={}, call_id={}, generation={}, reason={}",
                        info.device_id,
                        info.channel_id,
                        info.talk_id,
                        info.ssrc,
                        info.call_id,
                        info.generation,
                        info.last_error
                            .as_deref()
                            .unwrap_or("close deadline expired")
                    );
                } else {
                    debug!(
                        "ignore stale talk closing event: talk_id={}, generation={}",
                        talk_id, generation
                    );
                }
            }
            ScheduleKey::Register(TimeScheduleKey::CatalogSubscription(
                device_id,
                generation,
            )) => {
                let _ = inner
                    .event_tx
                    .try_send(Event::RefreshCatalogSubscription(device_id, generation))
                    .hand_log(|msg| error!("{msg}"));
            }
            ScheduleKey::Register(TimeScheduleKey::ServerHeart(domain_id)) => {
                let _ = inner
                    .event_tx
                    .try_send(Event::ServerHeart(domain_id))
                    .hand_log(|msg| error!("{msg}"));
            }
            ScheduleKey::Register(TimeScheduleKey::OutSession(_)) => {}
            ScheduleKey::Transaction(key) => trans_keys.push(key),
            ScheduleKey::GeneralCache(key) => cache_keys.push(key),
        }
    }

    if !trans_keys.is_empty() {
        TransactionContext::handle_timeout_keys(trans_keys);
    }
    if !cache_keys.is_empty() {
        GeneralCache::purge_expired_keys(cache_keys);
    }
}
