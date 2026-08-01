use std::sync::Arc;

use base::exception::GlobalResultExt;
use base::log::{debug, error, warn};
use base::tokio::select;
use base::tokio::sync::Semaphore;
use base::tokio::sync::mpsc::Receiver;
use base::tokio_util::sync::CancellationToken;
use base::tokio_util::task::TaskTracker;

use crate::gb::sip::subscription;
use crate::register::core::{Inner, Register, TimeScheduleKey};
use crate::register::schedule::ScheduleKey;
use crate::service::{broadcast_close, hook_serv, stream_close};
use crate::state::session::Cache as GeneralCache;

const MAX_WORKER_POOL: usize = 128;

#[derive(Clone, Eq, PartialEq)]
pub enum Event {
    RefreshCatalogSubscription(Arc<str>, u64),
    OutSession(u64),
}

pub async fn schedule_event(
    inner: Arc<Inner>,
    mut event_rx: Receiver<Event>,
    cancel_token: CancellationToken,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_WORKER_POOL));
    let tasks = TaskTracker::new();
    loop {
        select! {
            biased;
            _ = cancel_token.cancelled() => {
                debug!("register scheduler task exiting after cancellation");
                break;
            }
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
            open = handle_rx_event(
                &mut event_rx,
                semaphore.clone(),
                &tasks,
                &cancel_token,
            ) => {
                if !open {
                    if cancel_token.is_cancelled() {
                        debug!("register event channel closed during shutdown");
                    } else {
                        error!("register event channel closed unexpectedly; scheduler task exiting");
                    }
                    break;
                }
            }
        }
    }
    tasks.close();
    tasks.wait().await;
}

async fn handle_rx_event(
    rx: &mut Receiver<Event>,
    semaphore: Arc<Semaphore>,
    tasks: &TaskTracker,
    cancel: &CancellationToken,
) -> bool {
    let Some(event) = rx.recv().await else {
        return false;
    };
    let permit = select! {
        permit = semaphore.acquire_owned() => {
            let Ok(permit) = permit.hand_log(|msg| error!("{msg}")) else {
                return false;
            };
            permit
        }
        _ = cancel.cancelled() => return false,
    };
    tasks.spawn(async move {
        let _permit = permit;
        hand_event(event).await;
    });
    true
}

async fn hand_event(event: Event) {
    match event {
        Event::RefreshCatalogSubscription(device_id, generation) => {
            let _ = subscription::refresh_catalog_subscription(device_id, generation).await;
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
                    Register::close_removed_session(&device_id, &session, "lease_expired");
                    Register::close_tcp_if_needed(&session);
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
            ScheduleKey::Register(TimeScheduleKey::BroadcastClosing(broadcast_id, generation)) => {
                broadcast_close::force_cleanup(
                    broadcast_id.as_ref(),
                    generation,
                    "close deadline expired",
                );
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
            ScheduleKey::Register(TimeScheduleKey::PlaybackPresenceExpiry(
                playback_id,
                generation,
            )) => {
                crate::service::playback_presence::expire(playback_id, generation).await;
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
    use super::{Event, handle_rx_event};
    use base::tokio::runtime::Builder;
    use base::tokio::sync::{Semaphore, mpsc};
    use base::tokio_util::sync::CancellationToken;
    use base::tokio_util::task::TaskTracker;
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
                let tasks = TaskTracker::new();
                let cancel = CancellationToken::new();

                assert!(
                    !handle_rx_event(&mut rx, Arc::new(Semaphore::new(1)), &tasks, &cancel,).await
                );
            });
    }

    #[test]
    fn cancellation_interrupts_event_worker_backpressure() {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let semaphore = Arc::new(Semaphore::new(1));
                let _permit = semaphore.clone().acquire_owned().await.expect("permit");
                let (tx, mut rx) = mpsc::channel(1);
                tx.send(Event::OutSession(1)).await.expect("event");
                let tasks = TaskTracker::new();
                let cancel = CancellationToken::new();
                cancel.cancel();

                assert!(!handle_rx_event(&mut rx, semaphore, &tasks, &cancel).await);
            });
    }
}
