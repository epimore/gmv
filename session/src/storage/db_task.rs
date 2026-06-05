use base::log::{error, warn};
use base::once_cell::sync::OnceCell;
use base::tokio::select;
use base::tokio::sync::mpsc::error::TrySendError;
use base::tokio::sync::mpsc::{self, Receiver, Sender};
use base::tokio_util::sync::CancellationToken;

use crate::storage::entity::{GmvDevice, GmvDeviceChannel, GmvDeviceExt};

const DB_TASK_QUEUE_SIZE: usize = 8192;

static DB_TASK_TX: OnceCell<Sender<DbTask>> = OnceCell::new();

#[derive(Debug)]
pub enum DbTask {
    UpsertDevice(GmvDevice),
    ExpireDeviceOnline {
        device_id: String,
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

pub fn init(cancel: CancellationToken) {
    if DB_TASK_TX.get().is_some() {
        return;
    }

    let (tx, rx) = mpsc::channel(DB_TASK_QUEUE_SIZE);
    if DB_TASK_TX.set(tx).is_err() {
        return;
    }

    base::tokio::spawn(run(rx, cancel));
}

pub fn submit(task: DbTask) {
    let Some(tx) = DB_TASK_TX.get() else {
        warn!("session db task queue is not initialized; task dropped");
        return;
    };

    match tx.try_send(task) {
        Ok(_) => {}
        Err(TrySendError::Full(task)) => {
            error!("session db task queue is full; task dropped: {task:?}");
        }
        Err(TrySendError::Closed(task)) => {
            error!("session db task queue is closed; task dropped: {task:?}");
        }
    }
}

async fn run(mut rx: Receiver<DbTask>, cancel: CancellationToken) {
    loop {
        select! {
            item = rx.recv() => {
                let Some(task) = item else {
                    break;
                };
                handle_task(task).await;
            }
            _ = cancel.cancelled() => break,
        }
    }
}

async fn handle_task(task: DbTask) {
    match task {
        DbTask::UpsertDevice(device) => {
            if let Err(err) = device.insert_single_gmv_device_by_register().await {
                error!("upsert gmv device failed: {err:?}");
            }
        }
        DbTask::ExpireDeviceOnline { device_id } => {
            if let Err(err) = GmvDevice::expire_online_by_device_id(&device_id).await {
                error!("expire gmv device online failed: device_id={device_id}, err={err:?}");
            }
        }
        DbTask::TouchDeviceHeartbeat { device_id } => {
            if let Err(err) = GmvDevice::refresh_online_expire_time_by_device_id(&device_id).await {
                error!(
                    "refresh gmv device online expire failed: device_id={device_id}, err={err:?}"
                );
            }
        }
        DbTask::UpdateDeviceExtInfo(items) => {
            if let Err(err) = GmvDeviceExt::update_gmv_device_ext_info(items).await {
                error!("update gmv device ext info failed: {err:?}");
            }
        }
        DbTask::InsertDeviceCatalog { device_id, items } => {
            if let Err(err) = GmvDeviceChannel::insert_gmv_device_channel(&device_id, items).await {
                error!("insert gmv device channel failed: device_id={device_id}, err={err:?}");
            }
        }
    }
}
