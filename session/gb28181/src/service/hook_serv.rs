use std::ops::Sub;
use std::path::Path;

use base::bytes::Bytes;
use base::chrono::{Local, TimeZone};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::serde_json;
use gmv_domain::info::obj::{
    InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RegisterStreamInfo, StreamPlayInfo,
    StreamRecordInfo, StreamState, TalkClosedEvent, UnknownStreamEvent,
};

use crate::gb::SessionConf;
use crate::service::{KEY_STREAM_IN, dialog_recovery, stream_close, talk_close};
use crate::state;
use crate::state::DownloadConf;
use crate::storage::dialog_session::SipDialogSessionRepository;
use crate::storage::entity::{GmvFileInfo, GmvRecord};

pub async fn stream_register(register_stream_info: RegisterStreamInfo) {
    let key_stream_in_id = format!(
        "{}{}",
        KEY_STREAM_IN, register_stream_info.base_stream_info.stream_id
    );
    if register_stream_info.code == 200 {
        touch_durable_dialog(&register_stream_info.base_stream_info.stream_id).await;
        let _ = state::session::Cache::notify_stream_wait(
            &key_stream_in_id,
            Some(register_stream_info.base_stream_info),
        );
    } else {
        let _ = state::session::Cache::notify_stream_wait(&key_stream_in_id, None);
    }
}

pub fn stream_input_timeout(stream_state: StreamState) -> InTimeoutEventRes {
    let stream_id = stream_state.base_stream_info.stream_id;
    if state::session::Cache::stream_is_closing(&stream_id) {
        return InTimeoutEventRes::CloseAll;
    }
    stream_close::begin(stream_id);
    InTimeoutEventRes::CloseAll
}

pub fn on_play(stream_play_info: StreamPlayInfo) -> bool {
    let gmv_token = stream_play_info.token;
    let stream_id = stream_play_info.base_stream_info.stream_id;
    let accepted = state::session::Cache::stream_map_contains_token(&stream_id, &gmv_token);
    if accepted {
        base::tokio::spawn(async move {
            touch_durable_dialog(&stream_id).await;
        });
    }
    accepted
}

pub async fn off_play(stream_play_info: StreamPlayInfo) {
    let stream_id = stream_play_info.base_stream_info.stream_id;
    let gmv_token = stream_play_info.token;
    state::session::Cache::stream_map_remove(&stream_id, Some(&gmv_token));
}

pub async fn stream_idle(out_stream_info: OutputStreamInfo) -> OutputEventRes {
    if out_stream_info.user_count > 0 {
        return OutputEventRes::CloseMuxer;
    }

    stream_close::begin(out_stream_info.base_stream_info.stream_id);

    OutputEventRes::CloseAll
}

pub async fn stream_unknown(event: UnknownStreamEvent) -> bool {
    if !state::StreamConf::get_stream_conf()
        .node_map
        .contains_key(&event.media_node_id)
    {
        warn!(
            "unknown stream callback rejected: media_node={}, ssrc={}, reason=unconfigured node",
            event.media_node_id, event.ssrc
        );
        return false;
    }

    if event.ssrc >= 2_000_000_000 || event.ssrc % 10_000 == 0 {
        warn!(
            "unknown stream callback rejected: media_node={}, ssrc={}, reason=invalid protocol SSRC",
            event.media_node_id, event.ssrc
        );
        return false;
    }

    let domain_id = SessionConf::get_session_by_conf().domain_id;
    let ssrc = format!("{:010}", event.ssrc);
    let realtime_prefix = match crate::storage::ssrc_sequence::prefix(
        &domain_id,
        crate::storage::ssrc_sequence::SsrcKind::Realtime,
    ) {
        Ok(prefix) => prefix,
        Err(err) => {
            warn!("unknown stream callback rejected: {err}");
            return false;
        }
    };
    let history_prefix = crate::storage::ssrc_sequence::prefix(
        &domain_id,
        crate::storage::ssrc_sequence::SsrcKind::History,
    )
    .unwrap_or_default();
    if !ssrc.starts_with(&realtime_prefix) && !ssrc.starts_with(&history_prefix) {
        warn!(
            "unknown stream callback uses legacy SSRC prefix: media_node={}, ssrc={}",
            event.media_node_id, ssrc
        );
    }

    let stream_ids =
        state::session::Cache::stream_ids_by_node_ssrc(&event.media_node_id, event.ssrc);
    if stream_ids.len() == 1 {
        stream_close::begin(stream_ids[0].clone());
        return true;
    }
    if stream_ids.len() > 1 {
        warn!(
            "unknown stream callback ambiguous in memory: media_node={}, ssrc={}, matches={}",
            event.media_node_id,
            ssrc,
            stream_ids.len()
        );
        return false;
    }

    let Ok(first_seen_at_ms) = i64::try_from(event.first_seen_at_ms) else {
        warn!("unknown stream callback rejected: invalid first_seen_at_ms");
        return false;
    };
    let Some(first_seen_at) = Local
        .timestamp_millis_opt(first_seen_at_ms)
        .single()
        .map(|value| value.naive_local())
    else {
        warn!("unknown stream callback rejected: invalid first_seen_at_ms");
        return false;
    };
    let sessions = match SipDialogSessionRepository::find_active_by_media_ssrc_before(
        &domain_id,
        &event.media_node_id,
        &ssrc,
        first_seen_at,
        Local::now().naive_local(),
    )
    .await
    {
        Ok(sessions) => sessions,
        Err(err) => {
            warn!(
                "unknown stream durable lookup failed: media_node={}, ssrc={}, err={err}",
                event.media_node_id, ssrc
            );
            return false;
        }
    };
    if sessions.len() != 1 {
        warn!(
            "unknown stream durable match is not unique: media_node={}, ssrc={}, matches={}",
            event.media_node_id,
            ssrc,
            sessions.len()
        );
        return false;
    }
    let session = &sessions[0];
    if let Err(err) = dialog_recovery::recover_dialog(session).await {
        warn!(
            "unknown stream durable recovery failed: stream_id={}, ssrc={}, err={err}",
            session.stream_id, ssrc
        );
        return false;
    }
    stream_close::begin(session.stream_id.clone());
    true
}

async fn touch_durable_dialog(stream_id: &str) {
    let Ok(Some(session)) = SipDialogSessionRepository::find_by_stream_id(stream_id).await else {
        return;
    };
    let now = base::chrono::Local::now().naive_local();
    match SipDialogSessionRepository::cas_touch(
        stream_id,
        &session.signal_node_id,
        session.version,
        now,
        now + base::chrono::Duration::hours(8),
    )
    .await
    {
        Ok(true) | Ok(false) => {}
        Err(err) => base::log::warn!(
            "refresh durable dialog activity failed: stream_id={stream_id}; err={err}"
        ),
    }
}

pub async fn end_record(stream_record_info: StreamRecordInfo) -> GlobalResult<()> {
    let Some(path_file_name) = stream_record_info.path_file_name else {
        return Ok(());
    };
    let (abs_path, dir_path, biz_id, extension) = get_path(&path_file_name)?;
    let Some(mut record) = GmvRecord::query_gmv_record_by_biz_id(&biz_id).await? else {
        return Ok(());
    };
    if stream_record_info.file_size == 0 || stream_record_info.timestamp == 0 {
        record.state = 3;
    } else {
        let total_secs = record.et.sub(record.st).num_seconds();
        if total_secs <= 0 {
            record.state = 3;
        } else {
            let per = (stream_record_info.timestamp as i64) * 1000 / total_secs;
            if per > 98 {
                record.state = 1;
            } else {
                record.state = 2;
            }
        }
    }
    record.lt = Local::now().naive_local();
    record.update_gmv_record_by_biz_id().await?;
    let file_info = GmvFileInfo {
        id: None,
        device_id: record.device_id,
        channel_id: record.channel_id,
        biz_time: Some(Local::now().naive_local()),
        biz_id,
        file_type: Some(1),
        file_size: stream_record_info.file_size,
        file_name: record.biz_id,
        file_format: Some(extension),
        dir_path,
        abs_path,
        note: None,
        is_del: Some(0),
        create_time: Some(Local::now().naive_local()),
    };
    GmvFileInfo::insert_gmv_file_info(vec![file_info]).await?;
    Ok(())
}

pub async fn talk_closed(event: TalkClosedEvent) -> bool {
    let closed = talk_close::begin(event.talk_id.clone());
    if !closed {
        error!(
            "talk_closed cleanup skipped: talk_id={}, reason={}",
            event.talk_id, event.reason
        );
    }
    closed
}

fn get_path(path_file_name: &str) -> GlobalResult<(String, String, String, String)> {
    let path = Path::new(path_file_name);
    let biz_id = path
        .file_stem()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_str()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_string();
    let extension = path
        .extension()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_str()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_string();
    let p_path = path
        .parent()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let l_path1 = p_path
        .file_name()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let p_path = p_path
        .parent()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let l_path2 = p_path
        .file_name()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let d_path = DownloadConf::get_download_conf().storage_path;
    let dir_path = Path::new(&d_path)
        .join(l_path2)
        .join(l_path1)
        .to_str()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_string();
    let abs_path = p_path
        .to_str()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_string();
    Ok((abs_path, dir_path, biz_id, extension))
}
