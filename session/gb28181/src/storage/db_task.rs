use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use base::exception::GlobalError;
use base::log::{error, info, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::once_cell::sync::OnceCell;
use base::tokio::select;
use base::tokio::sync::mpsc::error::TrySendError;
use base::tokio::sync::mpsc::{self, Receiver, Sender};
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;

use crate::storage::entity::{GmvDevice, GmvDeviceChannel, GmvDeviceExt};

const DB_TASK_QUEUE_SIZE: usize = 8192;

static DB_TASK_TX: OnceCell<Sender<DbTask>> = OnceCell::new();
static DB_TASK_QUEUE_FULL_CLOCK: OnceLock<Instant> = OnceLock::new();
static DB_TASK_QUEUE_FULL_ACTIVE: AtomicBool = AtomicBool::new(false);
static DB_TASK_QUEUE_FULL_TOTAL: AtomicU64 = AtomicU64::new(0);
static DB_TASK_QUEUE_FULL_SINCE_LOG: AtomicU64 = AtomicU64::new(0);
static DB_TASK_QUEUE_FULL_LAST_LOG_SEC: AtomicU64 = AtomicU64::new(0);
static DB_TASK_QUEUE_CLOSED_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub enum DbTask {
    UpsertDevice(GmvDevice),
    ExpireDeviceOnline {
        device_id: String,
    },
    CloseDeviceEpoch {
        device_id: String,
        registration_epoch_id: Option<String>,
    },
    TouchDeviceHeartbeat {
        device_id: String,
    },
    UpdateDeviceExtInfo(Vec<(String, String)>),
    InsertDeviceCatalog {
        device_id: String,
        items: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, Copy)]
enum DbOperation {
    UpsertDevice,
    ExpireDeviceOnline,
    CloseDeviceEpoch,
    TouchDeviceHeartbeat,
    UpdateDeviceExtInfo,
    InsertDeviceCatalog,
}

impl DbOperation {
    const COUNT: usize = 6;

    fn index(self) -> usize {
        self as usize
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::UpsertDevice => "upsert_device",
            Self::ExpireDeviceOnline => "expire_device_online",
            Self::CloseDeviceEpoch => "close_device_epoch",
            Self::TouchDeviceHeartbeat => "touch_device_heartbeat",
            Self::UpdateDeviceExtInfo => "update_device_ext_info",
            Self::InsertDeviceCatalog => "insert_device_catalog",
        }
    }
}

impl DbTask {
    fn operation(&self) -> DbOperation {
        match self {
            Self::UpsertDevice(_) => DbOperation::UpsertDevice,
            Self::ExpireDeviceOnline { .. } => DbOperation::ExpireDeviceOnline,
            Self::CloseDeviceEpoch { .. } => DbOperation::CloseDeviceEpoch,
            Self::TouchDeviceHeartbeat { .. } => DbOperation::TouchDeviceHeartbeat,
            Self::UpdateDeviceExtInfo(_) => DbOperation::UpdateDeviceExtInfo,
            Self::InsertDeviceCatalog { .. } => DbOperation::InsertDeviceCatalog,
        }
    }
}

pub fn init(runtime: &GlobalRuntime, cancel: CancellationToken) -> Result<(), GlobalError> {
    if DB_TASK_TX.get().is_some() {
        return Ok(());
    }

    let (tx, rx) = mpsc::channel(DB_TASK_QUEUE_SIZE);
    if DB_TASK_TX.set(tx).is_err() {
        return Ok(());
    }

    runtime.spawn("session-db-worker", run(rx, cancel))?;
    Ok(())
}

pub fn submit(task: DbTask) {
    let operation = task.operation();
    let Some(tx) = DB_TASK_TX.get() else {
        warn!("session db task queue is not initialized; task dropped");
        return;
    };

    match tx.try_send(task) {
        Ok(_) => {
            if tx.capacity() >= DB_TASK_QUEUE_SIZE / 2
                && DB_TASK_QUEUE_FULL_ACTIVE.swap(false, Ordering::AcqRel)
            {
                let total = DB_TASK_QUEUE_FULL_TOTAL.swap(0, Ordering::AcqRel);
                let suppressed = total.saturating_sub(1);
                DB_TASK_QUEUE_FULL_SINCE_LOG.store(0, Ordering::Release);
                info!(
                    "session db task queue state changed: state=ready, previous_state=full, outcome=recovered, dropped_total={total}, suppressed={suppressed}"
                );
            }
        }
        Err(TrySendError::Full(_)) => {
            record_queue_full(operation);
        }
        Err(TrySendError::Closed(_)) => {
            base::log::trace!(
                "session db task queue rejected task: state=closed, operation={}",
                operation.as_str()
            );
            if !DB_TASK_QUEUE_CLOSED_LOGGED.swap(true, Ordering::AcqRel) {
                error!(
                    "session db task queue state changed: state=closed, operation={}, outcome=task_dropped",
                    operation.as_str()
                );
            }
        }
    }
}

fn record_queue_full(operation: DbOperation) {
    base::log::trace!(
        "session db task queue rejected task: state=full, operation={}",
        operation.as_str()
    );
    let now_sec = DB_TASK_QUEUE_FULL_CLOCK
        .get_or_init(Instant::now)
        .elapsed()
        .as_secs();
    let total = DB_TASK_QUEUE_FULL_TOTAL
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1);
    DB_TASK_QUEUE_FULL_SINCE_LOG.fetch_add(1, Ordering::AcqRel);
    if !DB_TASK_QUEUE_FULL_ACTIVE.swap(true, Ordering::AcqRel) {
        DB_TASK_QUEUE_FULL_LAST_LOG_SEC.store(now_sec, Ordering::Release);
        DB_TASK_QUEUE_FULL_SINCE_LOG.store(0, Ordering::Release);
        error!(
            "session db task queue state changed: state=full, previous_state=ready, operation={}, outcome=task_dropped",
            operation.as_str()
        );
        return;
    }
    let last_log_sec = DB_TASK_QUEUE_FULL_LAST_LOG_SEC.load(Ordering::Acquire);
    if now_sec.saturating_sub(last_log_sec) < 60
        || DB_TASK_QUEUE_FULL_LAST_LOG_SEC
            .compare_exchange(last_log_sec, now_sec, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let window_total = DB_TASK_QUEUE_FULL_SINCE_LOG.swap(0, Ordering::AcqRel);
    error!(
        "session db task queue remains full: state=full, outcome=ongoing, latest_operation={}, dropped_total={total}, dropped_since_last_summary={}, suppressed={}",
        operation.as_str(),
        window_total,
        window_total.saturating_sub(1)
    );
}

async fn run(mut rx: Receiver<DbTask>, cancel: CancellationToken) {
    let mut failure_episodes: [FailureEpisode; DbOperation::COUNT] =
        std::array::from_fn(|_| FailureEpisode::default());
    loop {
        select! {
            item = rx.recv() => {
                let Some(task) = item else {
                    error!("session DB worker exiting because task queue closed unexpectedly");
                    GlobalRuntime::request_shutdown_with_error();
                    break;
                };
                process_task(task, &mut failure_episodes).await;
            }
            _ = cancel.cancelled() => {
                rx.close();
                let mut drained = 0usize;
                let mut failed = false;
                while let Some(task) = rx.recv().await {
                    failed |= !process_task(task, &mut failure_episodes).await;
                    drained = drained.saturating_add(1);
                }
                if failed {
                    GlobalRuntime::request_shutdown_with_error();
                }
                base::log::debug!(
                    "session DB worker exited after cancellation: outcome={}, drained_tasks={drained}",
                    if failed { "incomplete" } else { "drained" }
                );
                break;
            },
        }
    }
}

async fn process_task(
    task: DbTask,
    failure_episodes: &mut [FailureEpisode; DbOperation::COUNT],
) -> bool {
    match handle_task(task).await {
        Ok(operation) => {
            record_db_success(operation, &mut failure_episodes[operation.index()]);
            true
        }
        Err((operation, err)) => {
            record_db_failure(operation, err, &mut failure_episodes[operation.index()]);
            false
        }
    }
}

async fn handle_task(task: DbTask) -> Result<DbOperation, (DbOperation, GlobalError)> {
    let operation = task.operation();
    let result = match task {
        DbTask::UpsertDevice(device) => device
            .insert_single_gmv_device_by_register()
            .await
            .map(|_| ()),
        DbTask::ExpireDeviceOnline { device_id } => {
            GmvDevice::expire_online_by_device_id(&device_id)
                .await
                .map(|_| ())
        }
        DbTask::CloseDeviceEpoch {
            device_id,
            registration_epoch_id,
        } => GmvDevice::close_registration_epoch(&device_id, registration_epoch_id.as_deref())
            .await
            .map(|_| ()),
        DbTask::TouchDeviceHeartbeat { device_id } => {
            GmvDevice::refresh_online_expire_time_by_device_id(&device_id)
                .await
                .map(|_| ())
        }
        DbTask::UpdateDeviceExtInfo(items) => GmvDeviceExt::update_gmv_device_ext_info(items)
            .await
            .map(|_| ()),
        DbTask::InsertDeviceCatalog { device_id, items } => {
            GmvDeviceChannel::insert_gmv_device_channel(&device_id, items)
                .await
                .map(|_| ())
        }
    };
    result.map(|_| operation).map_err(|err| (operation, err))
}

fn record_db_failure(operation: DbOperation, err: GlobalError, episode: &mut FailureEpisode) {
    base::log::trace!(
        "session db operation failed: operation={}, err={err:?}",
        operation.as_str()
    );
    match episode.record_failure(Instant::now()) {
        EpisodeDecision::Started => error!(
            "session db operation state changed: state=failed, previous_state=ready, operation={}, reason=database_error",
            operation.as_str()
        ),
        EpisodeDecision::Summary {
            total,
            since_last_summary,
            suppressed,
            duration,
        } => error!(
            "session db operation remains failed: state=failed, outcome=ongoing, operation={}, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
            operation.as_str(),
            duration.as_millis()
        ),
        EpisodeDecision::Suppressed => {}
        EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
    }
}

fn record_db_success(operation: DbOperation, episode: &mut FailureEpisode) {
    if let EpisodeDecision::Recovered {
        total,
        suppressed,
        duration,
    } = episode.record_success(Instant::now())
    {
        info!(
            "session db operation state changed: state=ready, previous_state=failed, outcome=recovered, operation={}, total_failures={total}, suppressed={suppressed}, duration_ms={}",
            operation.as_str(),
            duration.as_millis()
        );
    }
}
