use std::time::Duration;

use crate::gb::sip::command as sip_command;
use crate::service::{KEY_SNAPSHOT_IMAGE, SNAPSHOT_IDLE_EXPIRES};
use crate::state;
use crate::state::model::SnapshotImage;
use crate::utils::edge_token;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use base::tokio::sync::mpsc;
use base::tokio::time::Instant;

pub async fn snapshot_image(info: SnapshotImage) -> GlobalResult<String> {
    let (token, session_id) = edge_token::build_token_session_id(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
    )?;
    let push_url = crate::state::GuardConf::get_or_default().picture_upload_url();
    let url = format!("{}/{}", push_url.trim_end_matches('/'), token);
    let count = info.count.unwrap_or(1);
    if count == 0 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "snapshot count must be greater than zero",
            |msg| error!("{msg}"),
        ));
    }
    let (tx, mut rx) = mpsc::channel(8);
    let timeout = snapshot_idle_timeout();
    let when = Instant::now() + timeout;
    let key = rebuild_snapshot_wait_key(&session_id);
    state::session::Cache::insert_snapshot_wait(key.clone(), when, tx);

    if let Err(err) = sip_command::snapshot_image_call(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
        count,
        info.interval.unwrap_or(1),
        &url,
        &session_id,
    )
    .await
    {
        state::session::Cache::remove_state(&key);
        return Err(err);
    }

    if let Some(true) = rx.recv().await {
        state::session::Cache::remove_state(&key);
        return Ok(session_id);
    }

    Err(GlobalError::new_biz_error(
        BaseErrorCode::Timeout.code(),
        "快照失败:设备不支持或响应超时",
        |msg| error!("{msg}"),
    ))
}

pub fn rebuild_snapshot_wait_key(session_id: &str) -> String {
    format!("{}{}", KEY_SNAPSHOT_IMAGE, session_id)
}

fn snapshot_idle_timeout() -> Duration {
    Duration::from_secs(SNAPSHOT_IDLE_EXPIRES)
}

mod test {
    #[test]
    fn test_path() {}
}
