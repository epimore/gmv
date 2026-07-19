use std::sync::Arc;

use base::exception::GlobalResultExt;
use base::log::{debug, error, warn};
use base::tokio;
use base::tokio::select;
use base::tokio::sync::Semaphore;
use base::tokio::sync::mpsc::Receiver;
use base::tokio_util::sync::CancellationToken;

use crate::gb::sip::subscription;
use crate::register::core::{Inner, Register, TimeScheduleKey};
use crate::register::schedule::ScheduleKey;
use crate::service::{hook_serv, stream_close, talk_close};
use crate::state::session::Cache as GeneralCache;
use crate::storage::db_task::{self, DbTask};

const MAX_WORKER_POOL: usize = 128;

#[derive(Clone, Eq, PartialEq)]
pub enum Event {
    DeviceOffline(Arc<str>),
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
                    None => {
                        if cancel_token.is_cancelled() {
                            debug!("register scheduler channel closed during shutdown");
                        } else {
                            error!("register scheduler channel closed unexpectedly");
                        }
                        break;
                    },
                }
            }
            open = handle_rx_event(&mut event_rx, semaphore.clone()) => {
                if !open {
                    if cancel_token.is_cancelled() {
                        debug!("register event channel closed during shutdown");
                    } else {
                        error!("register event channel closed unexpectedly; scheduler task exiting");
                    }
                    break;
                }
            }
            _ = cancel_token.cancelled() => {
                debug!("register scheduler task exiting after cancellation");
                break;
            },
        }
    }
}

async fn handle_rx_event(rx: &mut Receiver<Event>, semaphore: Arc<Semaphore>) -> bool {
    let Some(event) = rx.recv().await else {
        return false;
    };
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
    true
}

async fn hand_event(event: Event) {
    match event {
        Event::DeviceOffline(device_id) => {
            db_task::submit(DbTask::ExpireDeviceOnline {
                device_id: device_id.to_string(),
            });
        }
        Event::RefreshCatalogSubscription(device_id, generation) => {
            let _ = subscription::refresh_catalog_subscription(device_id, generation)
                .await
                .hand_log(|msg| warn!("refresh catalog subscription failed: {msg}"));
        }
        Event::OutSession(_) => {}
    }
}

async fn on_time_schedule(
    inner: &Inner,
    batch: Vec<crate::register::schedule::ScheduleEvent<ScheduleKey>>,
) {
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
                stream_close::force_cleanup(
                    stream_id.as_ref(),
                    generation,
                    "close deadline expired",
                );
            }
            ScheduleKey::Register(TimeScheduleKey::TalkClosing(talk_id, generation)) => {
                talk_close::force_cleanup(talk_id.as_ref(), generation, "close deadline expired");
            }
            ScheduleKey::Register(TimeScheduleKey::CatalogSubscription(device_id, generation)) => {
                let _ = inner
                    .event_tx
                    .try_send(Event::RefreshCatalogSubscription(device_id, generation))
                    .hand_log(|msg| error!("{msg}"));
            }
            ScheduleKey::Register(TimeScheduleKey::PlaybackPauseExpiry(stream_id, generation)) => {
                hook_serv::expire_playback_pause(stream_id, generation).await;
            }
            ScheduleKey::Register(TimeScheduleKey::OutSession(_)) => {}
            ScheduleKey::GeneralCache(key) => cache_keys.push(key),
        }
    }

    if !cache_keys.is_empty() {
        GeneralCache::purge_expired_keys(cache_keys);
    }
}

#[cfg(test)]
mod tests {
    use super::handle_rx_event;
    use base::tokio::runtime::Builder;
    use base::tokio::sync::{Semaphore, mpsc};
    use std::sync::Arc;

    #[test]
    fn closed_event_channel_is_reported_to_owner_loop() {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let (tx, mut rx) = mpsc::channel(1);
                drop(tx);

                assert!(!handle_rx_event(&mut rx, Arc::new(Semaphore::new(1))).await);
            });
    }
}
