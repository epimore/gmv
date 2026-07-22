use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base_db::sqlx::Acquire;
use sqlx::FromRow;

use crate::storage::db;

pub const ROW_QUERY: i64 = 0;
pub const ROW_SEGMENT: i64 = 1;
pub const STATUS_QUERYING: i64 = 0;
pub const STATUS_READY: i64 = 1;
pub const STATUS_EMPTY: i64 = 2;
pub const STATUS_FAILED: i64 = 3;
pub const QUERY_COOLDOWN_MS: i64 = 5 * 60 * 1000;
pub const QUERY_STALE_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Clone, FromRow)]
struct RecordRow {
    id: i64,
    device_id: String,
    channel_id: String,
    batch_id: String,
    item_no: i64,
    row_type: i64,
    status: Option<i64>,
    start_time_sec: i64,
    end_time_sec: i64,
    remote_device_id: Option<String>,
    name: Option<String>,
    file_path: Option<String>,
    address: Option<String>,
    secrecy: Option<i64>,
    record_type: Option<String>,
    recorder_id: Option<String>,
    file_size: Option<i64>,
    create_time: i64,
}

#[derive(Debug, Clone, FromRow)]
struct ExistsRow {
    value: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RecordQueryBatch {
    pub batch_id: String,
    pub status: i64,
    pub start_time_sec: i64,
    pub end_time_sec: i64,
    pub created_at_ms: i64,
}

impl RecordQueryBatch {
    pub fn status_name(&self) -> &'static str {
        match self.status {
            STATUS_QUERYING => "QUERYING",
            STATUS_READY => "READY",
            STATUS_EMPTY => "EMPTY",
            STATUS_FAILED => "FAILED",
            _ => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecordSegment {
    pub segment_id: i64,
    pub batch_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub remote_device_id: String,
    pub name: String,
    pub file_path: String,
    pub address: String,
    pub start_time_sec: i64,
    pub end_time_sec: i64,
    pub secrecy: i64,
    pub record_type: String,
    pub recorder_id: String,
    pub file_size: i64,
}

#[derive(Debug, Clone, Default)]
pub struct RecordState {
    pub current_batch: Option<RecordQueryBatch>,
    pub attempt_batch: Option<RecordQueryBatch>,
    pub segments: Vec<RecordSegment>,
    pub next_query_at_ms: i64,
    pub server_time_ms: i64,
}

impl RecordState {
    pub async fn get(device_id: &str, channel_id: &str, now_ms: i64) -> GlobalResult<Self> {
        db::execute!(
            "UPDATE gb28181_device_record_segment SET status=3 WHERE device_id=? AND channel_id=? AND row_type=0 AND status=0 AND create_time<=?",
            device_id,
            channel_id,
            now_ms.saturating_sub(QUERY_STALE_MS),
        )
        .hand_log(|msg| error!("{msg}"))?;
        let rows = db::fetch_all_as!(
            RecordRow,
            r#"SELECT id,device_id,channel_id,batch_id,item_no,
                      CAST(row_type AS SIGNED) AS row_type,CAST(status AS SIGNED) AS status,
                      start_time_sec,end_time_sec,remote_device_id,name,file_path,address,
                      secrecy,record_type,recorder_id,file_size,create_time
                 FROM gb28181_device_record_segment
                WHERE device_id=? AND channel_id=? AND row_type=0
                ORDER BY create_time DESC,id DESC"#,
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        let current_row = rows
            .iter()
            .find(|row| matches!(row.status, Some(STATUS_READY) | Some(STATUS_EMPTY)));
        let attempt_batch = rows
            .iter()
            .find(|row| matches!(row.status, Some(STATUS_QUERYING) | Some(STATUS_FAILED)))
            .map(query_batch);
        let current_batch = current_row.map(query_batch);
        let segments =
            if let Some(current) = current_row.filter(|row| row.status == Some(STATUS_READY)) {
                db::fetch_all_as!(
                    RecordRow,
                    r#"SELECT id,device_id,channel_id,batch_id,item_no,
                          CAST(row_type AS SIGNED) AS row_type,CAST(status AS SIGNED) AS status,
                          start_time_sec,end_time_sec,remote_device_id,name,file_path,address,
                          secrecy,record_type,recorder_id,file_size,create_time
                     FROM gb28181_device_record_segment
                    WHERE device_id=? AND channel_id=? AND batch_id=? AND row_type=1
                    ORDER BY start_time_sec,end_time_sec,item_no"#,
                    device_id,
                    channel_id,
                    &current.batch_id,
                )
                .hand_log(|msg| error!("{msg}"))?
                .into_iter()
                .map(record_segment)
                .collect()
            } else {
                Vec::new()
            };
        let next_query_at_ms = rows
            .iter()
            .filter(|row| row.status != Some(STATUS_FAILED))
            .map(|row| row.create_time)
            .max()
            .map(|created| created.saturating_add(QUERY_COOLDOWN_MS))
            .unwrap_or_default();
        Ok(Self {
            current_batch,
            attempt_batch,
            segments,
            next_query_at_ms,
            server_time_ms: now_ms,
        })
    }

    pub async fn claim(
        device_id: &str,
        channel_id: &str,
        batch_id: &str,
        start_time_sec: i64,
        end_time_sec: i64,
        now_ms: i64,
    ) -> GlobalResult<Self> {
        let exists = db::fetch_optional_as!(
            ExistsRow,
            "SELECT 1 AS value FROM gb28181_device_channel WHERE device_id=? AND channel_id=?",
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        if !exists.is_some_and(|row| row.value == 1) {
            return Err(biz_error("record_query_channel_not_found"));
        }

        db::execute!(
            "UPDATE gb28181_device_record_segment SET status=3 WHERE device_id=? AND channel_id=? AND row_type=0 AND status=0 AND create_time<=?",
            device_id,
            channel_id,
            now_ms.saturating_sub(QUERY_STALE_MS),
        )
        .hand_log(|msg| error!("{msg}"))?;
        let querying = db::fetch_optional_as!(
            ExistsRow,
            "SELECT 1 AS value FROM gb28181_device_record_segment WHERE device_id=? AND channel_id=? AND row_type=0 AND status=0",
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        if querying.is_some_and(|row| row.value == 1) {
            return Err(biz_error("record_query_in_progress"));
        }
        let recent = db::fetch_optional_as!(
            RecordRow,
            r#"SELECT id,device_id,channel_id,batch_id,item_no,
                      CAST(row_type AS SIGNED) AS row_type,CAST(status AS SIGNED) AS status,
                      start_time_sec,end_time_sec,remote_device_id,name,file_path,address,
                      secrecy,record_type,recorder_id,file_size,create_time
                 FROM gb28181_device_record_segment
                WHERE device_id=? AND channel_id=? AND row_type=0 AND status<>3
                ORDER BY create_time DESC,id DESC LIMIT 1"#,
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        if recent.is_some_and(|row| now_ms < row.create_time.saturating_add(QUERY_COOLDOWN_MS)) {
            return Err(biz_error("record_query_too_frequent"));
        }
        db::execute!(
            "DELETE FROM gb28181_device_record_segment WHERE device_id=? AND channel_id=? AND row_type=0 AND status=3",
            device_id,
            channel_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        db::execute!(
            r#"INSERT INTO gb28181_device_record_segment
               (device_id,channel_id,batch_id,item_no,row_type,status,start_time_sec,end_time_sec,create_time)
               VALUES (?,?,?,0,0,0,?,?,?)"#,
            device_id,
            channel_id,
            batch_id,
            start_time_sec,
            end_time_sec,
            now_ms,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Self::get(device_id, channel_id, now_ms).await
    }

    pub async fn complete(
        device_id: &str,
        channel_id: &str,
        batch_id: &str,
        segments: &[RecordSegment],
        created_at_ms: i64,
    ) -> GlobalResult<()> {
        macro_rules! complete_on {
            ($pool:expr) => {{
                let mut tx = $pool.begin().await.hand_log(|msg| error!("{msg}"))?;
                base_db::sqlx::query(
                    "DELETE FROM gb28181_device_record_segment WHERE device_id=? AND channel_id=? AND batch_id<>?",
                )
                .bind(device_id)
                .bind(channel_id)
                .bind(batch_id)
                .execute(&mut *tx)
                .await
                .hand_log(|msg| error!("{msg}"))?;
                for (index, segment) in segments.iter().enumerate() {
                    base_db::sqlx::query(
                        r#"INSERT INTO gb28181_device_record_segment
                           (device_id,channel_id,batch_id,item_no,row_type,status,start_time_sec,end_time_sec,
                            remote_device_id,name,file_path,address,secrecy,record_type,recorder_id,file_size,create_time)
                           VALUES (?,?,?, ?,1,NULL, ?,?,?,?,?,?,?,?,?,?,?)"#,
                    )
                    .bind(device_id)
                    .bind(channel_id)
                    .bind(batch_id)
                    .bind(i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1))
                    .bind(segment.start_time_sec)
                    .bind(segment.end_time_sec)
                    .bind(empty_to_none(&segment.remote_device_id))
                    .bind(empty_to_none(&segment.name))
                    .bind(empty_to_none(&segment.file_path))
                    .bind(empty_to_none(&segment.address))
                    .bind(segment.secrecy)
                    .bind(empty_to_none(&segment.record_type))
                    .bind(empty_to_none(&segment.recorder_id))
                    .bind(segment.file_size)
                    .bind(created_at_ms)
                    .execute(&mut *tx)
                    .await
                    .hand_log(|msg| error!("{msg}"))?;
                }
                let status = if segments.is_empty() { STATUS_EMPTY } else { STATUS_READY };
                let updated = base_db::sqlx::query(
                    "UPDATE gb28181_device_record_segment SET status=? WHERE device_id=? AND channel_id=? AND batch_id=? AND row_type=0 AND status=0",
                )
                .bind(status)
                .bind(device_id)
                .bind(channel_id)
                .bind(batch_id)
                .execute(&mut *tx)
                .await
                .hand_log(|msg| error!("{msg}"))?
                .rows_affected();
                if updated != 1 {
                    return Err(biz_error("record_query_state_changed"));
                }
                tx.commit().await.hand_log(|msg| error!("{msg}"))?;
            }};
        }
        match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => complete_on!(db::mysql_pool()),
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => complete_on!(db::sqlite_pool()),
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        Ok(())
    }

    pub async fn fail(device_id: &str, channel_id: &str, batch_id: &str) -> GlobalResult<()> {
        db::execute!(
            "DELETE FROM gb28181_device_record_segment WHERE device_id=? AND channel_id=? AND batch_id=? AND row_type=1",
            device_id,
            channel_id,
            batch_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        db::execute!(
            "UPDATE gb28181_device_record_segment SET status=3 WHERE device_id=? AND channel_id=? AND batch_id=? AND row_type=0 AND status=0",
            device_id,
            channel_id,
            batch_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}

fn query_batch(row: &RecordRow) -> RecordQueryBatch {
    debug_assert_eq!(row.row_type, ROW_QUERY);
    debug_assert_eq!(row.item_no, 0);
    RecordQueryBatch {
        batch_id: row.batch_id.clone(),
        status: row.status.unwrap_or(STATUS_FAILED),
        start_time_sec: row.start_time_sec,
        end_time_sec: row.end_time_sec,
        created_at_ms: row.create_time,
    }
}

fn record_segment(row: RecordRow) -> RecordSegment {
    debug_assert_eq!(row.row_type, ROW_SEGMENT);
    RecordSegment {
        segment_id: row.id,
        batch_id: row.batch_id,
        device_id: row.device_id,
        channel_id: row.channel_id,
        remote_device_id: row.remote_device_id.unwrap_or_default(),
        name: row.name.unwrap_or_default(),
        file_path: row.file_path.unwrap_or_default(),
        address: row.address.unwrap_or_default(),
        start_time_sec: row.start_time_sec,
        end_time_sec: row.end_time_sec,
        secrecy: row.secrecy.unwrap_or_default(),
        record_type: row.record_type.unwrap_or_default(),
        recorder_id: row.recorder_id.unwrap_or_default(),
        file_size: row.file_size.unwrap_or_default(),
    }
}

fn empty_to_none(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn biz_error(message: &str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidRequest.code(), message, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_status_names_are_stable() {
        for (status, expected) in [
            (STATUS_QUERYING, "QUERYING"),
            (STATUS_READY, "READY"),
            (STATUS_EMPTY, "EMPTY"),
            (STATUS_FAILED, "FAILED"),
        ] {
            assert_eq!(
                RecordQueryBatch {
                    status,
                    ..Default::default()
                }
                .status_name(),
                expected
            );
        }
    }
}
