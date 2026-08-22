use std::path::{Component, Path, PathBuf};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error};
use gmv_domain::info::output::OutputEnum;
use uuid::Uuid;

use crate::service::api_serv;
use crate::state::model::{PlayBackModel, StreamQo, TransMode};
use crate::state::session::AccessMode;
use crate::state::{DownloadConf, session};
use crate::storage::recording::{self, CloudRecordStart, CloudRecording};

const MAX_DURATION_SEC: i64 = 7_200;

pub struct CreateInput<'a> {
    pub request_id: &'a str,
    pub session_node_id: &'a str,
    pub device_id: &'a str,
    pub channel_id: &'a str,
    pub requested_by: &'a str,
    pub start_time_sec: i64,
    pub end_time_sec: i64,
}

pub async fn create(input: CreateInput<'_>) -> GlobalResult<CloudRecording> {
    validate_create(&input)?;
    let setup_lock = crate::state::session::Cache::stream_setup_lock(
        input.device_id,
        input.channel_id,
        AccessMode::Down,
    );
    let _setup_guard = setup_lock.lock().await;
    if let Some(existing) = recording::find_by_request_id(input.request_id).await? {
        return Ok(existing);
    }
    if recording::running_record_exists(input.device_id, input.channel_id).await? {
        return Err(biz(
            BaseErrorCode::AlreadyExists,
            "CLOUD_RECORDING_ALREADY_ACTIVE",
        ));
    }
    ensure_storage_ready()?;

    let task_id = format!("cr-{}", Uuid::new_v4());
    recording::create_cloud_record(CloudRecordStart {
        task_id: &task_id,
        request_id: input.request_id,
        session_node_id: input.session_node_id,
        device_id: input.device_id,
        channel_id: input.channel_id,
        requested_by: input.requested_by,
        st_epoch_sec: input.start_time_sec,
        et_epoch_sec: input.end_time_sec,
    })
    .await?;

    let token = Uuid::new_v4().to_string();
    let result = api_serv::download_for_task_with_setup_lock(
        PlayBackModel {
            device_id: input.device_id.to_string(),
            channel_id: Some(input.channel_id.to_string()),
            trans_mode: Some(TransMode::Udp),
            custom_media_config: None,
            st: u32::try_from(input.start_time_sec).unwrap_or_default(),
            et: u32::try_from(input.end_time_sec).unwrap_or_default(),
        },
        token,
        &task_id,
    )
    .await;
    match result {
        Ok(info) => {
            let stream_node = session::Cache::stream_map_query_node_ssrc(&info.streamId)
                .map(|value| value.0)
                .unwrap_or_default();
            recording::bind_running(&task_id, &info.streamId, &stream_node).await?;
            debug!(
                "cloud recording started: action=create, outcome=running, task_id={task_id}, stream_id={}, device_id={}, channel_id={}",
                info.streamId, input.device_id, input.channel_id
            );
        }
        Err(err) => {
            recording::mark_failed(&task_id, "DEVICE_DOWNLOAD_REJECTED", "设备录像下载启动失败")
                .await?;
            error!(
                "cloud recording start failed: action=create, outcome=failed, task_id={task_id}, device_id={}, channel_id={}, err={err}",
                input.device_id, input.channel_id
            );
        }
    }
    recording::get(&task_id)
        .await?
        .ok_or_else(|| biz(BaseErrorCode::NotFound, "CLOUD_RECORDING_NOT_FOUND"))
}

pub async fn get_with_progress(task_id: &str) -> GlobalResult<CloudRecording> {
    let mut record = recording::get(task_id)
        .await?
        .ok_or_else(|| biz(BaseErrorCode::NotFound, "CLOUD_RECORDING_NOT_FOUND"))?;
    refresh_progress(&record).await;
    if matches!(
        recording::normalized_status(&record),
        recording::STATUS_RUNNING | recording::STATUS_STOPPING
    ) {
        record = recording::get(task_id).await?.unwrap_or(record);
    }
    Ok(record)
}

pub async fn list(
    device_id: &str,
    channel_id: &str,
    page: u32,
    page_size: u32,
    include_deleted: bool,
) -> GlobalResult<(Vec<CloudRecording>, u64)> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);
    let (mut records, total) =
        recording::list(device_id, channel_id, page, page_size, include_deleted).await?;
    for record in &records {
        refresh_progress(record).await;
    }
    for record in &mut records {
        if matches!(
            recording::normalized_status(record),
            recording::STATUS_RUNNING | recording::STATUS_STOPPING
        ) && let Some(latest) = recording::get(&record.task_id).await?
        {
            *record = latest;
        }
    }
    Ok((records, total))
}

pub async fn stop(task_id: &str) -> GlobalResult<CloudRecording> {
    let record = get_with_progress(task_id).await?;
    let status = recording::normalized_status(&record);
    if !matches!(
        status,
        recording::STATUS_STARTING | recording::STATUS_RUNNING | recording::STATUS_STOPPING
    ) {
        return Ok(record);
    }
    if status != recording::STATUS_STOPPING {
        recording::claim_stop(task_id).await?;
    }
    if let Some(stream_id) = record.stream_id.filter(|value| !value.is_empty()) {
        api_serv::download_stop(stream_id, record.stream_node.as_deref(), String::new()).await?;
    } else {
        recording::finish_stopped_without_file(task_id).await?;
    }
    get_with_progress(task_id).await
}

pub async fn delete(task_id: &str) -> GlobalResult<CloudRecording> {
    let previous = recording::claim_delete(task_id)
        .await?
        .ok_or_else(|| biz(BaseErrorCode::NotFound, "CLOUD_RECORDING_NOT_FOUND"))?;
    if previous == recording::STATUS_DELETED {
        return get_with_progress(task_id).await;
    }
    let result = delete_files(task_id).await;
    match result {
        Ok(()) => recording::finish_delete(task_id).await?,
        Err(err) => {
            recording::rollback_delete(task_id, &previous).await?;
            return Err(err);
        }
    }
    get_with_progress(task_id).await
}

pub fn resolve_file_path(
    root: &Path,
    dir_path: &str,
    file_name: &str,
    format: &str,
) -> GlobalResult<PathBuf> {
    let relative = Path::new(dir_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || file_name.contains(['/', '\\'])
    {
        return Err(biz(BaseErrorCode::InvalidRequest, "STORAGE_PATH_INVALID"));
    }
    let extension = format.trim_start_matches('.');
    let file = if extension.is_empty() {
        file_name.to_string()
    } else {
        format!("{file_name}.{extension}")
    };
    Ok(root.join(relative).join(file))
}

pub fn storage_root() -> GlobalResult<PathBuf> {
    std::fs::canonicalize(&DownloadConf::get_download_conf().storage_path)
        .map_err(|_| biz(BaseErrorCode::Network, "STORAGE_UNAVAILABLE"))
}

async fn refresh_progress(record: &CloudRecording) {
    if !matches!(
        recording::normalized_status(record),
        recording::STATUS_RUNNING | recording::STATUS_STOPPING
    ) {
        return;
    }
    let Some(stream_id) = record.stream_id.as_ref().filter(|value| !value.is_empty()) else {
        return;
    };
    if let Ok(info) = api_serv::download_info_by_stream_id(
        StreamQo {
            stream_id: stream_id.clone(),
            media_type: Some(OutputEnum::LocalMp4),
        },
        record.stream_node.as_deref(),
        String::new(),
    )
    .await
    {
        let _ = recording::update_progress(
            &record.task_id,
            u64::from(info.timestamp).saturating_mul(1_000),
            info.file_size,
        )
        .await;
    }
}

fn validate_create(input: &CreateInput<'_>) -> GlobalResult<()> {
    if input.request_id.trim().is_empty()
        || input.session_node_id.trim().is_empty()
        || input.device_id.trim().is_empty()
        || input.channel_id.trim().is_empty()
        || input.start_time_sec <= 0
        || input.start_time_sec >= input.end_time_sec
    {
        return Err(biz(
            BaseErrorCode::InvalidRequest,
            "CLOUD_RECORDING_RANGE_REQUIRED",
        ));
    }
    if input.end_time_sec - input.start_time_sec > MAX_DURATION_SEC {
        return Err(biz(
            BaseErrorCode::InvalidRequest,
            "CLOUD_RECORDING_RANGE_TOO_LARGE",
        ));
    }
    Ok(())
}

fn ensure_storage_ready() -> GlobalResult<()> {
    let root = storage_root()?;
    let conf = DownloadConf::get_download_conf();
    let available = fs2::available_space(&root)
        .map_err(|_| biz(BaseErrorCode::Network, "STORAGE_UNAVAILABLE"))?;
    if available <= conf.min_free_bytes {
        return Err(biz(BaseErrorCode::Network, "STORAGE_LOW_SPACE"));
    }
    Ok(())
}

async fn delete_files(task_id: &str) -> GlobalResult<()> {
    let Some(file) = recording::file(task_id).await? else {
        return Ok(());
    };
    let root = storage_root()?;
    let format = file.file_format.as_deref().unwrap_or("mp4");
    let path = resolve_file_path(&root, &file.dir_path, &file.file_name, format)?;
    for candidate in [
        path.clone(),
        path.with_extension("mp4.part"),
        path.with_extension("mp4.json"),
    ] {
        match base::tokio::fs::remove_file(&candidate).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(biz(BaseErrorCode::Network, "STORAGE_DELETE_FAILED")),
        }
    }
    Ok(())
}

fn biz(code: BaseErrorCode, message: &str) -> GlobalError {
    GlobalError::new_biz_error(code.code(), message, |msg| error!("{msg}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{CreateInput, resolve_file_path, validate_create};

    fn input(end_time_sec: i64) -> CreateInput<'static> {
        CreateInput {
            request_id: "request-1",
            session_node_id: "session-1",
            device_id: "34020000001320000001",
            channel_id: "34020000001320000002",
            requested_by: "operator",
            start_time_sec: 1_000,
            end_time_sec,
        }
    }

    #[test]
    fn two_hour_range_is_inclusive() {
        assert!(validate_create(&input(8_200)).is_ok());
        assert!(validate_create(&input(8_201)).is_err());
    }

    #[test]
    fn file_path_rejects_escape_and_file_name_separators() {
        let root = Path::new("/srv/cloud-recordings");
        assert!(resolve_file_path(root, "20260720/mp4", "task-1", "mp4").is_ok());
        assert!(resolve_file_path(root, "../outside", "task-1", "mp4").is_err());
        assert!(resolve_file_path(root, "20260720/mp4", "../task-1", "mp4").is_err());
        assert!(resolve_file_path(root, "/outside", "task-1", "mp4").is_err());
    }
}
