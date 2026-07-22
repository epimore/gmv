use base::chrono::{Local, NaiveDateTime, TimeZone};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use sqlx::FromRow;

use crate::state::DownloadConf;
use crate::storage::db;

pub const STATUS_STARTING: &str = "STARTING";
pub const STATUS_RUNNING: &str = "RUNNING";
pub const STATUS_STOPPING: &str = "STOPPING";
pub const STATUS_COMPLETED: &str = "COMPLETED";
pub const STATUS_STOPPED: &str = "STOPPED";
pub const STATUS_PARTIAL: &str = "PARTIAL";
pub const STATUS_FAILED: &str = "FAILED";
pub const STATUS_DELETING: &str = "DELETING";
pub const STATUS_DELETED: &str = "DELETED";

pub const FILE_NONE: &str = "NONE";
pub const FILE_WRITING: &str = "WRITING";
pub const FILE_READY: &str = "READY";
pub const FILE_MISSING: &str = "MISSING";
pub const FILE_DELETED: &str = "DELETED";

#[derive(Debug, Clone)]
pub struct RecordStart<'a> {
    pub biz_id: &'a str,
    pub device_id: &'a str,
    pub channel_id: &'a str,
    pub st_epoch_sec: i64,
    pub et_epoch_sec: i64,
    pub speed: u32,
    pub stream_app_name: &'a str,
}

#[derive(Debug, Clone)]
pub struct CloudRecordStart<'a> {
    pub task_id: &'a str,
    pub request_id: &'a str,
    pub session_node_id: &'a str,
    pub device_id: &'a str,
    pub channel_id: &'a str,
    pub requested_by: &'a str,
    pub st_epoch_sec: i64,
    pub et_epoch_sec: i64,
}

#[derive(Debug, Clone)]
pub struct RecordFinish<'a> {
    pub biz_id: &'a str,
    pub reported_state: u8,
    pub file_size: u64,
    pub record_duration_sec: u64,
    pub file_format: &'a str,
    pub dir_path: &'a str,
    pub abs_path: &'a str,
}

#[derive(Debug, Clone, FromRow)]
pub struct CloudRecording {
    pub task_id: String,
    pub request_id: Option<String>,
    pub session_node_id: Option<String>,
    pub stream_id: Option<String>,
    pub device_id: String,
    pub channel_id: String,
    pub user_id: Option<String>,
    pub st: Option<String>,
    pub et: Option<String>,
    pub ct: Option<String>,
    pub state: Option<i64>,
    pub status: Option<String>,
    pub file_state: Option<String>,
    pub recorded_duration_ms: i64,
    pub current_size_bytes: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub lt: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct CloudRecordingFile {
    pub file_size: Option<i64>,
    pub file_name: String,
    pub file_format: Option<String>,
    pub dir_path: String,
    pub abs_path: Option<String>,
}

#[derive(Debug, FromRow)]
struct RecordMeta {
    device_id: String,
    channel_id: String,
    st: String,
    et: String,
    status: Option<String>,
}

pub async fn running_record_exists(device_id: &str, channel_id: &str) -> GlobalResult<bool> {
    let row: Option<(i32,)> = db::fetch_optional_as!(
        (i32,),
        "SELECT 1 FROM gb28181_record WHERE device_id=? AND channel_id=? AND (status IN ('STARTING','RUNNING','STOPPING') OR (status IS NULL AND state=0)) LIMIT 1",
        device_id,
        channel_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(row.is_some())
}

pub async fn find_by_request_id(request_id: &str) -> GlobalResult<Option<CloudRecording>> {
    db::fetch_optional_as!(CloudRecording, "SELECT r.biz_id AS task_id,r.request_id,r.session_node_id,r.stream_id,r.device_id,r.channel_id,r.user_id,CAST(r.st AS CHAR) AS st,CAST(r.et AS CHAR) AS et,CAST(r.ct AS CHAR) AS ct,CAST(r.state AS SIGNED) AS state,r.status,r.file_state,CAST(COALESCE(r.recorded_duration_ms,0) AS SIGNED) AS recorded_duration_ms,CAST(COALESCE(r.current_size_bytes,0) AS SIGNED) AS current_size_bytes,CAST(r.started_at AS CHAR) AS started_at,CAST(r.finished_at AS CHAR) AS finished_at,CAST(r.lt AS CHAR) AS lt,r.error_code,r.error_message FROM gb28181_record r WHERE r.request_id=?", request_id,)
        .hand_log(|msg| error!("{msg}"))
}

pub async fn get(task_id: &str) -> GlobalResult<Option<CloudRecording>> {
    db::fetch_optional_as!(CloudRecording, "SELECT r.biz_id AS task_id,r.request_id,r.session_node_id,r.stream_id,r.device_id,r.channel_id,r.user_id,CAST(r.st AS CHAR) AS st,CAST(r.et AS CHAR) AS et,CAST(r.ct AS CHAR) AS ct,CAST(r.state AS SIGNED) AS state,r.status,r.file_state,CAST(COALESCE(r.recorded_duration_ms,0) AS SIGNED) AS recorded_duration_ms,CAST(COALESCE(r.current_size_bytes,0) AS SIGNED) AS current_size_bytes,CAST(r.started_at AS CHAR) AS started_at,CAST(r.finished_at AS CHAR) AS finished_at,CAST(r.lt AS CHAR) AS lt,r.error_code,r.error_message FROM gb28181_record r WHERE r.biz_id=?", task_id,)
        .hand_log(|msg| error!("{msg}"))
}

pub async fn list(
    device_id: &str,
    channel_id: &str,
    page: u32,
    page_size: u32,
    include_deleted: bool,
) -> GlobalResult<(Vec<CloudRecording>, u64)> {
    let offset = i64::from(page.saturating_sub(1).saturating_mul(page_size));
    let limit = i64::from(page_size);
    let (rows, count) = if include_deleted {
        let rows = db::fetch_all_as!(CloudRecording, "SELECT r.biz_id AS task_id,r.request_id,r.session_node_id,r.stream_id,r.device_id,r.channel_id,r.user_id,CAST(r.st AS CHAR) AS st,CAST(r.et AS CHAR) AS et,CAST(r.ct AS CHAR) AS ct,CAST(r.state AS SIGNED) AS state,r.status,r.file_state,CAST(COALESCE(r.recorded_duration_ms,0) AS SIGNED) AS recorded_duration_ms,CAST(COALESCE(r.current_size_bytes,0) AS SIGNED) AS current_size_bytes,CAST(r.started_at AS CHAR) AS started_at,CAST(r.finished_at AS CHAR) AS finished_at,CAST(r.lt AS CHAR) AS lt,r.error_code,r.error_message FROM gb28181_record r WHERE r.device_id=? AND r.channel_id=? ORDER BY r.ct DESC LIMIT ? OFFSET ?", device_id, channel_id, limit, offset,)
            .hand_log(|msg| error!("{msg}"))?;
        let count = db::fetch_optional_as!(
            (i64,),
            "SELECT COUNT(*) FROM gb28181_record WHERE device_id=? AND channel_id=?",
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?
        .unwrap_or((0,))
        .0;
        (rows, count)
    } else {
        let rows = db::fetch_all_as!(CloudRecording, "SELECT r.biz_id AS task_id,r.request_id,r.session_node_id,r.stream_id,r.device_id,r.channel_id,r.user_id,CAST(r.st AS CHAR) AS st,CAST(r.et AS CHAR) AS et,CAST(r.ct AS CHAR) AS ct,CAST(r.state AS SIGNED) AS state,r.status,r.file_state,CAST(COALESCE(r.recorded_duration_ms,0) AS SIGNED) AS recorded_duration_ms,CAST(COALESCE(r.current_size_bytes,0) AS SIGNED) AS current_size_bytes,CAST(r.started_at AS CHAR) AS started_at,CAST(r.finished_at AS CHAR) AS finished_at,CAST(r.lt AS CHAR) AS lt,r.error_code,r.error_message FROM gb28181_record r WHERE r.device_id=? AND r.channel_id=? AND COALESCE(r.status,'')<>'DELETED' ORDER BY r.ct DESC LIMIT ? OFFSET ?", device_id, channel_id, limit, offset,)
            .hand_log(|msg| error!("{msg}"))?;
        let count = db::fetch_optional_as!((i64,), "SELECT COUNT(*) FROM gb28181_record WHERE device_id=? AND channel_id=? AND COALESCE(status,'')<>'DELETED'", device_id, channel_id,)
            .hand_log(|msg| error!("{msg}"))?.unwrap_or((0,)).0;
        (rows, count)
    };
    Ok((rows, u64::try_from(count).unwrap_or_default()))
}

pub async fn file(task_id: &str) -> GlobalResult<Option<CloudRecordingFile>> {
    db::fetch_optional_as!(
        CloudRecordingFile,
        "SELECT CAST(file_size AS SIGNED) AS file_size,file_name,file_format,dir_path,abs_path FROM gb28181_file_info WHERE biz_id=? AND COALESCE(is_del,0)=0 ORDER BY id DESC LIMIT 1",
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))
}

pub async fn create_cloud_record(record: CloudRecordStart<'_>) -> GlobalResult<()> {
    let st = format_epoch(record.st_epoch_sec)?;
    let et = format_epoch(record.et_epoch_sec)?;
    let now = now_string();
    db::execute!(
        "INSERT INTO gb28181_record(biz_id,request_id,session_node_id,device_id,channel_id,user_id,st,et,speed,ct,state,lt,status,file_state,recorded_duration_ms,current_size_bytes,version) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        record.task_id,
        record.request_id,
        record.session_node_id,
        record.device_id,
        record.channel_id,
        record.requested_by,
        &st,
        &et,
        1_i64,
        &now,
        0_i64,
        &now,
        STATUS_STARTING,
        FILE_NONE,
        0_i64,
        0_i64,
        0_i64,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn bind_running(task_id: &str, stream_id: &str, stream_node: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET stream_id=?,stream_app_name=?,status=?,file_state=?,started_at=?,lt=?,version=version+1 WHERE biz_id=? AND status='STARTING'",
        stream_id,
        stream_node,
        STATUS_RUNNING,
        FILE_WRITING,
        &now,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn mark_failed(task_id: &str, code: &str, message: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET state=3,status=?,file_state=?,error_code=?,error_message=?,finished_at=?,lt=?,version=version+1 WHERE biz_id=? AND status IN ('STARTING','RUNNING','STOPPING')",
        STATUS_FAILED,
        FILE_NONE,
        code,
        message,
        &now,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn claim_stop(task_id: &str) -> GlobalResult<bool> {
    let now = now_string();
    let result = db::execute!(
        "UPDATE gb28181_record SET status=?,terminal_reason='user_stop',lt=?,version=version+1 WHERE biz_id=? AND status IN ('STARTING','RUNNING')",
        STATUS_STOPPING,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(result > 0)
}

pub async fn finish_stopped_without_file(task_id: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET state=2,status=?,file_state=?,finished_at=?,lt=?,version=version+1 WHERE biz_id=? AND status='STOPPING'",
        STATUS_STOPPED,
        FILE_NONE,
        &now,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn update_progress(task_id: &str, duration_ms: u64, size: u64) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET recorded_duration_ms=?,current_size_bytes=?,lt=?,version=version+1 WHERE biz_id=? AND status IN ('RUNNING','STOPPING')",
        i64::try_from(duration_ms).unwrap_or(i64::MAX),
        i64::try_from(size).unwrap_or(i64::MAX),
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn claim_delete(task_id: &str) -> GlobalResult<Option<String>> {
    let Some(record) = get(task_id).await? else {
        return Ok(None);
    };
    let status = normalized_status(&record);
    if !matches!(
        status,
        STATUS_COMPLETED | STATUS_STOPPED | STATUS_PARTIAL | STATUS_FAILED | STATUS_DELETED
    ) {
        return Err(invalid_state("CLOUD_RECORDING_NOT_TERMINAL"));
    }
    if status == STATUS_DELETED {
        return Ok(Some(STATUS_DELETED.to_string()));
    }
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET status=?,lt=?,version=version+1 WHERE biz_id=? AND status=?",
        STATUS_DELETING,
        &now,
        task_id,
        status,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(Some(status.to_string()))
}

pub async fn finish_delete(task_id: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_file_info SET is_del=1 WHERE biz_id=?",
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    db::execute!(
        "UPDATE gb28181_record SET status=?,file_state=?,deleted_at=?,lt=?,version=version+1 WHERE biz_id=? AND status IN ('DELETING','DELETED')",
        STATUS_DELETED,
        FILE_DELETED,
        &now,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn rollback_delete(task_id: &str, status: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET status=?,error_code='STORAGE_DELETE_FAILED',lt=?,version=version+1 WHERE biz_id=? AND status='DELETING'",
        status,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn mark_file_missing(task_id: &str) -> GlobalResult<()> {
    let now = now_string();
    db::execute!(
        "UPDATE gb28181_record SET file_state=?,error_code='CLOUD_RECORDING_FILE_MISSING',lt=?,version=version+1 WHERE biz_id=? AND file_state='READY'",
        FILE_MISSING,
        &now,
        task_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn start_record(record: RecordStart<'_>) -> GlobalResult<()> {
    if record.biz_id.is_empty() || record.stream_app_name.is_empty() {
        return Err(invalid_state("biz_id and stream_app_name are required"));
    }
    let st = format_epoch(record.st_epoch_sec)?;
    let et = format_epoch(record.et_epoch_sec)?;
    let now = now_string();
    db::execute!(
        "INSERT INTO gb28181_record(biz_id,device_id,channel_id,user_id,st,et,speed,ct,state,lt,stream_app_name,status,file_state,started_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        record.biz_id,
        record.device_id,
        record.channel_id,
        Option::<String>::None,
        &st,
        &et,
        i64::from(record.speed),
        &now,
        0_i64,
        &now,
        record.stream_app_name,
        STATUS_RUNNING,
        FILE_WRITING,
        &now,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn finish_record(file: RecordFinish<'_>) -> GlobalResult<bool> {
    if file.biz_id.is_empty() || file.dir_path.is_empty() {
        return Err(invalid_state("biz_id and dir_path are required"));
    }
    let Some(record) = db::fetch_optional_as!(
        RecordMeta,
        "SELECT device_id,channel_id,CAST(st AS CHAR) AS st,CAST(et AS CHAR) AS et,status FROM gb28181_record WHERE biz_id=?",
        file.biz_id,
    )
    .hand_log(|msg| error!("{msg}"))?
    else {
        return Ok(false);
    };
    if matches!(
        record.status.as_deref(),
        Some(STATUS_COMPLETED | STATUS_STOPPED | STATUS_PARTIAL | STATUS_FAILED | STATUS_DELETED)
    ) {
        return Ok(true);
    }
    let now = now_string();
    let legacy_state = record_state(
        file.reported_state,
        &record.st,
        &record.et,
        file.file_size,
        file.record_duration_sec,
    );
    let user_stopped = record.status.as_deref() == Some(STATUS_STOPPING);
    let (status, file_state) = terminal_state(legacy_state, user_stopped, file.file_size);
    let duration_ms = file.record_duration_sec.saturating_mul(1_000);
    db::execute!(
        "UPDATE gb28181_record SET state=?,status=?,file_state=?,recorded_duration_ms=?,current_size_bytes=?,finished_at=?,lt=?,version=version+1 WHERE biz_id=? AND status<>'DELETED'",
        i64::from(legacy_state),
        status,
        file_state,
        i64::try_from(duration_ms).unwrap_or(i64::MAX),
        i64::try_from(file.file_size).unwrap_or(i64::MAX),
        &now,
        &now,
        file.biz_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    let file_size = i64::try_from(file.file_size).unwrap_or(i64::MAX);
    let format = (!file.file_format.is_empty()).then_some(file.file_format);
    let abs_path = (!file.abs_path.is_empty()).then_some(file.abs_path);
    let storage_id = DownloadConf::get_download_conf().storage_id;
    db::execute!(
        "INSERT INTO gb28181_file_info(device_id,channel_id,biz_time,biz_id,file_type,file_size,file_name,file_format,dir_path,abs_path,storage_id,file_state,duration_ms,mime_type,note,is_del,create_time) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        &record.device_id,
        &record.channel_id,
        &now,
        file.biz_id,
        1_i32,
        file_size,
        file.biz_id,
        format,
        file.dir_path,
        abs_path,
        &storage_id,
        file_state,
        i64::try_from(duration_ms).unwrap_or(i64::MAX),
        "video/mp4",
        Option::<String>::None,
        0_i32,
        &now,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(true)
}

pub fn normalized_status(record: &CloudRecording) -> &str {
    record.status.as_deref().unwrap_or(match record.state {
        Some(1) => STATUS_COMPLETED,
        Some(2) => STATUS_PARTIAL,
        Some(3) => STATUS_FAILED,
        _ => STATUS_RUNNING,
    })
}

fn terminal_state(
    legacy_state: i32,
    user_stopped: bool,
    file_size: u64,
) -> (&'static str, &'static str) {
    let status = match (legacy_state, user_stopped) {
        (_, true) => STATUS_STOPPED,
        (1, false) => STATUS_COMPLETED,
        (2, false) => STATUS_PARTIAL,
        _ => STATUS_FAILED,
    };
    let file_state = if file_size > 0 && matches!(legacy_state, 1 | 2) {
        FILE_READY
    } else {
        FILE_NONE
    };
    (status, file_state)
}

pub fn epoch_sec(value: Option<&str>) -> i64 {
    value
        .and_then(|value| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok())
        .and_then(|value| Local.from_local_datetime(&value).single())
        .map(|value| value.timestamp())
        .unwrap_or_default()
}

pub fn epoch_ms(value: Option<&str>) -> i64 {
    epoch_sec(value).saturating_mul(1_000)
}

fn now_string() -> String {
    Local::now()
        .naive_local()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn format_epoch(epoch_sec: i64) -> GlobalResult<String> {
    Local
        .timestamp_opt(epoch_sec, 0)
        .single()
        .map(|value| value.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
        .ok_or_else(|| invalid_state("invalid record timestamp"))
}

fn invalid_state(message: &str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidRequest.code(), message, |msg| {
        error!("{msg}")
    })
}

fn record_state(
    reported_state: u8,
    st: &str,
    et: &str,
    file_size: u64,
    record_duration_sec: u64,
) -> i32 {
    if reported_state == 2 || reported_state == 3 {
        return i32::from(reported_state);
    }
    if file_size == 0 || record_duration_sec == 0 {
        return 3;
    }
    let expected = record_duration(st, et);
    if expected == 0 {
        return 2;
    }
    let percent = i64::try_from(record_duration_sec)
        .unwrap_or(i64::MAX)
        .saturating_mul(100)
        / expected;
    if percent >= 95 { 1 } else { 2 }
}

fn record_duration(st: &str, et: &str) -> i64 {
    let Ok(start) = NaiveDateTime::parse_from_str(st, "%Y-%m-%d %H:%M:%S") else {
        return 0;
    };
    let Ok(end) = NaiveDateTime::parse_from_str(et, "%Y-%m-%d %H:%M:%S") else {
        return 0;
    };
    (end - start).num_seconds().max(0)
}

#[cfg(test)]
mod tests {
    use super::{
        FILE_NONE, FILE_READY, STATUS_FAILED, STATUS_STOPPED, record_state, terminal_state,
    };

    const START: &str = "2026-07-10 18:00:00";
    const END: &str = "2026-07-10 18:01:40";

    #[test]
    fn reported_partial_is_not_promoted_by_duration() {
        assert_eq!(record_state(2, START, END, 1024, 100), 2);
    }

    #[test]
    fn reported_failure_is_not_promoted_by_duration() {
        assert_eq!(record_state(3, START, END, 1024, 100), 3);
    }

    #[test]
    fn reported_complete_still_uses_duration_quality() {
        assert_eq!(record_state(1, START, END, 1024, 100), 1);
        assert_eq!(record_state(1, START, END, 1024, 90), 2);
    }

    #[test]
    fn user_stop_without_playable_file_is_stopped_without_file() {
        assert_eq!(terminal_state(3, true, 1024), (STATUS_STOPPED, FILE_NONE));
    }

    #[test]
    fn non_user_failure_is_failed_without_file() {
        assert_eq!(terminal_state(3, false, 1024), (STATUS_FAILED, FILE_NONE));
        assert_eq!(terminal_state(2, false, 1024).1, FILE_READY);
    }
}
