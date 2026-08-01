use base::log::{error, info};

use crate::gb::sip::runtime_cache::SipRuntimeCache;
use crate::service::{broadcast_close, stream_close};
use crate::state::session::Cache;
use crate::storage::db_task::{self, DbTask};
use crate::storage::dialog_session::SipDialogSessionRepository;

pub fn close(device_id: &str, registration_epoch_id: Option<String>, reason: &'static str) {
    db_task::submit(DbTask::CloseDeviceEpoch {
        device_id: device_id.to_string(),
        registration_epoch_id: registration_epoch_id.clone(),
    });

    for stream_id in Cache::stream_ids_by_device(device_id) {
        stream_close::begin(stream_id);
    }
    for broadcast_id in Cache::broadcast_ids_by_device(device_id) {
        broadcast_close::begin(broadcast_id);
    }
    Cache::reset_device_state(device_id);

    let device_id = device_id.to_string();
    base::tokio::spawn(async move {
        let dialogs = match SipDialogSessionRepository::find_active_by_device_epoch(
            &device_id,
            registration_epoch_id.as_deref(),
        )
        .await
        {
            Ok(dialogs) => dialogs,
            Err(err) => {
                error!(
                    "registration epoch dialog lookup failed: device_id={device_id}, registration_epoch_id={registration_epoch_id:?}, reason={reason}, err={err}"
                );
                return;
            }
        };
        let active_dialog_count = dialogs.len();
        for dialog in dialogs {
            SipRuntimeCache::global()
                .remove_stream_indexes(&dialog.stream_id, Some(&dialog.call_id));
            stream_close::finalize_durable_dialog_as_orphan_for_epoch(
                "dialog",
                &dialog.stream_id,
                registration_epoch_id.as_deref(),
            )
            .await;
        }
        info!(
            "registration epoch closed: device_id={device_id}, registration_epoch_id={registration_epoch_id:?}, reason={reason}, active_dialog_count={active_dialog_count}"
        );
    });
}
