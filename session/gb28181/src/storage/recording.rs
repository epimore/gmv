use base::chrono::{Local, TimeZone};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use sqlx::FromRow;

use crate::storage::db;

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
pub struct RecordFinish<'a> {
    pub biz_id: &'a str,
    pub file_size: u64,
    pub record_duration_sec: u64,
    pub file_format: &'a str,
    pub dir_path: &'a str,
    pub abs_path: &'a str,
}

#[derive(Debug, FromRow)]
struct RecordMeta {
    device_id: String,
    channel_id: String,
    st: String,
    et: String,
}

pub async fn running_record_exists(device_id: &str, channel_id: &str) -> GlobalResult<bool> {
    let row: Option<(i32,)> = db::fetch_optional_as!(
        (i32,),
        "SELECT 1 FROM GB28181_RECORD WHERE STATE=0 AND DEVICE_ID=? AND CHANNEL_ID=? LIMIT 1",
        device_id,
        channel_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(row.is_some())
}

pub async fn start_record(record: RecordStart<'_>) -> GlobalResult<()> {
    if record.biz_id.is_empty() || record.stream_app_name.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "biz_id and stream_app_name are required",
            |msg| error!("{msg}"),
        ));
    }
    let st = format_epoch(record.st_epoch_sec)?;
    let et = format_epoch(record.et_epoch_sec)?;
    let now = now_string();
    db::execute!(
        "INSERT INTO GB28181_RECORD(BIZ_ID,DEVICE_ID,CHANNEL_ID,USER_ID,ST,ET,SPEED,CT,STATE,LT,STREAM_APP_NAME) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
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
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

pub async fn finish_record(file: RecordFinish<'_>) -> GlobalResult<bool> {
    if file.biz_id.is_empty() || file.dir_path.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "biz_id and dir_path are required",
            |msg| error!("{msg}"),
        ));
    }
    let Some(record) = db::fetch_optional_as!(
        RecordMeta,
        "SELECT DEVICE_ID AS device_id,CHANNEL_ID AS channel_id,ST AS st,ET AS et FROM GB28181_RECORD WHERE BIZ_ID=?",
        file.biz_id,
    )
    .hand_log(|msg| error!("{msg}"))? else {
        return Ok(false);
    };
    let now = now_string();
    let state = record_state(
        &record.st,
        &record.et,
        file.file_size,
        file.record_duration_sec,
    );
    db::execute!(
        "UPDATE GB28181_RECORD SET STATE=?,LT=? WHERE BIZ_ID=?",
        i64::from(state),
        &now,
        file.biz_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    let file_size = i64::try_from(file.file_size).unwrap_or(i64::MAX);
    let format = (!file.file_format.is_empty()).then_some(file.file_format);
    let abs_path = (!file.abs_path.is_empty()).then_some(file.abs_path);
    db::execute!(
        "INSERT INTO GB28181_FILE_INFO(DEVICE_ID,CHANNEL_ID,BIZ_TIME,BIZ_ID,FILE_TYPE,FILE_SIZE,FILE_NAME,FILE_FORMAT,DIR_PATH,ABS_PATH,NOTE,IS_DEL,CREATE_TIME) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
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
        Option::<String>::None,
        0_i32,
        &now,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(true)
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
        .ok_or_else(|| {
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "invalid record timestamp",
                |msg| error!("{msg}"),
            )
        })
}

fn record_state(st: &str, et: &str, file_size: u64, record_duration_sec: u64) -> i32 {
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
    let Ok(start) = base::chrono::NaiveDateTime::parse_from_str(st, "%Y-%m-%d %H:%M:%S") else {
        return 0;
    };
    let Ok(end) = base::chrono::NaiveDateTime::parse_from_str(et, "%Y-%m-%d %H:%M:%S") else {
        return 0;
    };
    (end - start).num_seconds().max(0)
}
