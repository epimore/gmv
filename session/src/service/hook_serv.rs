use crate::gb::handler::cmd::CmdStream;
use crate::service::KEY_STREAM_IN;
use crate::state;
use crate::state::DownloadConf;
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::utils::id_builder;
use base::bytes::Bytes;
use base::chrono::Local;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde_json;
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState};
use std::ops::Sub;
use std::path::Path;

pub async fn stream_register(base_stream_info: BaseStreamInfo) {
    let key_stream_in_id = format!("{}{}", KEY_STREAM_IN, base_stream_info.stream_id);
    if let Some((_, Some(tx))) = state::session::Cache::state_get(&key_stream_in_id) {
        let vec = serde_json::to_vec(&base_stream_info).unwrap();
        let bytes = Bytes::from(vec);
        let _ = tx.try_send(Some(bytes)).hand_log(|msg| error!("{msg}"));
    }
}

//gmv-stream接收流超时:还ssrc_sn,清理stream_map/device_map
pub fn stream_input_timeout(stream_state: StreamState) {
    let ssrc = stream_state.base_stream_info.rtp_info.ssrc;
    let ssrc_num = (ssrc % 10000) as u16;
    state::session::Cache::ssrc_sn_set(ssrc_num);
    let stream_id = stream_state.base_stream_info.stream_id;
    if let Some(am) = state::session::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
        state::session::Cache::stream_map_remove(&stream_id, None);
        if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
            state::session::Cache::device_map_remove(
                &device_id,
                Some((&channel_id, Some((am, &ssrc_str)))),
            );
        }
    }
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

//无人观看则关闭流
pub async fn stream_idle(base_stream_info: BaseStreamInfo) {
    let stream_id = base_stream_info.stream_id;
    let cst_info = state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
            let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag)
                .await;
        }
        if let Some(am) = state::session::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
            state::session::Cache::device_map_remove(
                &device_id,
                Some((&channel_id, Some((am, &ssrc_str)))),
            );
            state::session::Cache::stream_map_remove(&stream_id, None);
        }
        let ssrc = base_stream_info.rtp_info.ssrc;
        let ssrc_num = (ssrc % 10000) as u16;
        state::session::Cache::ssrc_sn_set(ssrc_num);
    }
}

pub async fn end_record(stream_record_info: StreamRecordInfo) {
    if let Some(path_file_name) = stream_record_info.path_file_name {
        if let Ok((abs_path, dir_path, biz_id, extension)) = get_path(&path_file_name) {
            if let Ok(Some(mut record)) = GmvRecord::query_gmv_record_by_biz_id(&biz_id).await {
                if stream_record_info.file_size == 0 || stream_record_info.timestamp == 0 {
                    record.state = 3;
                } else {
                    let total_secs = record.et.sub(record.st).num_seconds();
                    let per = (stream_record_info.timestamp as i64) * 1000 / total_secs;

                    if per > 98 {
                        record.state = 1;
                    } else {
                        record.state = 2;
                    }
                }
                record.lt = Local::now().naive_local();
                if let Ok(_) = record.update_gmv_record_by_biz_id().await {
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
                    let _ = GmvFileInfo::insert_gmv_file_info(vec![file_info]).await;
                }
            }
        }
    };
}

fn get_path(path_file_name: &str) -> GlobalResult<(String, String, String, String)> {
    let path = Path::new(&path_file_name);
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
