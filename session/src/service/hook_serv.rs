use std::ops::Sub;
use std::path::Path;

use base::bytes::Bytes;
use base::chrono::Local;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde_json;
use shared::info::obj::{
    InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RegisterStreamInfo, StreamPlayInfo,
    StreamRecordInfo, StreamState, TalkClosedEvent,
};

use crate::service::{KEY_STREAM_IN, stream_close, talk_close};
use crate::state;
use crate::state::DownloadConf;
use crate::storage::entity::{GmvFileInfo, GmvRecord};

pub async fn stream_register(register_stream_info: RegisterStreamInfo) {
    let key_stream_in_id = format!(
        "{}{}",
        KEY_STREAM_IN, register_stream_info.base_stream_info.stream_id
    );
    if register_stream_info.code == 200 {
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
    state::session::Cache::stream_map_contains_token(&stream_id, &gmv_token)
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
