use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use base::chrono::{DateTime, Local, LocalResult, NaiveDateTime, TimeZone};
use base::dashmap::DashMap;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::{Mutex, OwnedMutexGuard};
use base::tokio::time::{self, Instant};

use crate::gb::sip::runtime_cache::{RecordInfoKey, SipRuntimeCache};
use crate::gb::sip::xml::{RecordInfoItem, RecordInfoResponse};
use crate::register::core::Register;
use crate::storage::device_record::{RecordSegment, RecordState};

const MAX_RANGE_SEC: i64 = 366 * 24 * 60 * 60;
const CHUNK_RANGE_SEC: i64 = 31 * 24 * 60 * 60;
const RECORD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const RECORD_WAITER_TTL: Duration = Duration::from_secs(30);
const MAX_RESULT_ITEMS: usize = 50_000;
const MAX_BATCH_ID_CHARS: usize = 128;

static RECORD_QUERY_LOCKS: Lazy<DashMap<String, Arc<Mutex<()>>>> = Lazy::new(DashMap::new);

pub async fn start(
    device_id: String,
    channel_id: String,
    batch_id: String,
    start_time_sec: i64,
    end_time_sec: i64,
) -> GlobalResult<RecordState> {
    validate_request(&batch_id, start_time_sec, end_time_sec)?;
    let session = Register::get_connected_device_session(&device_id)
        .ok_or_else(|| biz_error("record_query_device_offline"))?;
    let key = format!("{device_id}/{channel_id}");
    let lock = RECORD_QUERY_LOCKS
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let guard = lock
        .try_lock_owned()
        .map_err(|_| biz_error("record_query_in_progress"))?;
    let now_ms = Local::now().timestamp_millis();
    let state = match RecordState::claim(
        &device_id,
        &channel_id,
        &batch_id,
        start_time_sec,
        end_time_sec,
        now_ms,
    )
    .await
    {
        Ok(state) => state,
        Err(err) => {
            drop(guard);
            cleanup_query_lock(&key);
            return Err(err);
        }
    };
    let registration_epoch_id = session.registration_epoch_id.clone();
    base::tokio::spawn(async move {
        run_background(
            guard,
            key,
            device_id,
            channel_id,
            batch_id,
            registration_epoch_id,
            start_time_sec,
            end_time_sec,
            now_ms,
        )
        .await;
    });
    Ok(state)
}

async fn run_background(
    guard: OwnedMutexGuard<()>,
    lock_key: String,
    device_id: String,
    channel_id: String,
    batch_id: String,
    registration_epoch_id: Option<String>,
    start_time_sec: i64,
    end_time_sec: i64,
    created_at_ms: i64,
) {
    let result = query_all_chunks(
        &device_id,
        &channel_id,
        registration_epoch_id,
        start_time_sec,
        end_time_sec,
    )
    .await;
    match result {
        Ok(segments) => {
            if let Err(err) =
                RecordState::complete(&device_id, &channel_id, &batch_id, &segments, created_at_ms)
                    .await
            {
                error!(
                    "record query persistence failed: action=query_record_info, outcome=failed, device_id={device_id}, channel_id={channel_id}, batch_id={batch_id}, err={err}"
                );
                let _ = RecordState::fail(&device_id, &channel_id, &batch_id).await;
            }
        }
        Err(err) => {
            warn!(
                "record query failed: action=query_record_info, outcome=failed, device_id={device_id}, channel_id={channel_id}, batch_id={batch_id}, err={err}"
            );
            if let Err(storage_error) = RecordState::fail(&device_id, &channel_id, &batch_id).await
            {
                error!(
                    "record query failure state persistence failed: device_id={device_id}, channel_id={channel_id}, batch_id={batch_id}, err={storage_error}"
                );
            }
        }
    }
    drop(guard);
    cleanup_query_lock(&lock_key);
}

fn cleanup_query_lock(key: &str) {
    RECORD_QUERY_LOCKS.remove_if(key, |_, lock| Arc::strong_count(lock) == 1);
}

async fn query_all_chunks(
    device_id: &str,
    channel_id: &str,
    registration_epoch_id: Option<String>,
    start_time_sec: i64,
    end_time_sec: i64,
) -> GlobalResult<Vec<RecordSegment>> {
    let mut chunks = Vec::new();
    let mut chunk_start = start_time_sec;
    while chunk_start < end_time_sec {
        let chunk_end = chunk_start
            .saturating_add(CHUNK_RANGE_SEC)
            .min(end_time_sec);
        chunks.push((chunk_start, chunk_end));
        chunk_start = chunk_end;
    }
    let mut all = Vec::new();
    let mut seen = HashSet::new();
    for (chunk_start, chunk_end) in chunks {
        let items = query_chunk(
            device_id,
            channel_id,
            registration_epoch_id.clone(),
            chunk_start,
            chunk_end,
        )
        .await?;
        for item in items {
            let segment = normalize_item(device_id, channel_id, item)?;
            let identity = segment_identity(&segment);
            if seen.insert(identity) {
                all.push(segment);
                if all.len() > MAX_RESULT_ITEMS {
                    return Err(biz_error("record_query_result_too_large"));
                }
            }
        }
    }
    all.sort_by_key(|segment| (segment.start_time_sec, segment.end_time_sec));
    Ok(all)
}

async fn query_chunk(
    device_id: &str,
    channel_id: &str,
    registration_epoch_id: Option<String>,
    start_time_sec: i64,
    end_time_sec: i64,
) -> GlobalResult<Vec<RecordInfoItem>> {
    let sn = crate::gb::sip::sequence::next_sn();
    let key = RecordInfoKey {
        parent_device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        sn,
    };
    let mut receiver = SipRuntimeCache::global().insert_record_info_waiter(
        key.clone(),
        registration_epoch_id,
        RECORD_WAITER_TTL,
    );
    let start_time = format_time(start_time_sec)?;
    let end_time = format_time(end_time_sec)?;
    if let Err(err) = crate::gb::sip::command::query_record_info(
        device_id,
        channel_id,
        sn,
        &start_time,
        &end_time,
    )
    .await
    {
        SipRuntimeCache::global().remove_record_info_waiter(&key);
        return Err(err);
    }

    let deadline = Instant::now() + RECORD_RESPONSE_TIMEOUT;
    let mut expected = None;
    let mut items = HashMap::<String, RecordInfoItem>::new();
    let mut seen_responses = HashSet::new();
    let mut received_count = 0usize;
    let result = async {
        loop {
            let response = time::timeout_at(deadline, receiver.recv())
                .await
                .map_err(|_| biz_error("record_query_timeout"))?
                .ok_or_else(|| biz_error("record_query_incomplete"))?;
            validate_response(channel_id, &response, &mut expected)?;
            if response.sum_num == 0 {
                return Ok(Vec::new());
            }
            if !seen_responses.insert(response_identity(&response)) {
                continue;
            }
            received_count = received_count.saturating_add(response.items.len());
            if received_count > expected.unwrap_or_default() {
                return Err(biz_error("record_query_invalid_response"));
            }
            for item in response.items {
                items.entry(item_identity(&item)).or_insert(item);
            }
            if received_count == expected.unwrap_or_default() {
                return Ok(items.into_values().collect());
            }
        }
    }
    .await;
    SipRuntimeCache::global().remove_record_info_waiter(&key);
    result
}

fn validate_response(
    channel_id: &str,
    response: &RecordInfoResponse,
    expected: &mut Option<usize>,
) -> GlobalResult<()> {
    if response.device_id != channel_id {
        return Err(biz_error("record_query_invalid_response"));
    }
    if response.list_num != response.items.len() {
        return Err(biz_error("record_query_invalid_response"));
    }
    if response.sum_num > MAX_RESULT_ITEMS {
        return Err(biz_error("record_query_result_too_large"));
    }
    match expected {
        Some(value) if *value != response.sum_num => {
            return Err(biz_error("record_query_invalid_response"));
        }
        None => *expected = Some(response.sum_num),
        _ => {}
    }
    Ok(())
}

fn normalize_item(
    device_id: &str,
    channel_id: &str,
    item: RecordInfoItem,
) -> GlobalResult<RecordSegment> {
    if !item.device_id.is_empty() && item.device_id != channel_id {
        return Err(biz_error("record_query_invalid_response"));
    }
    if item.device_id.chars().count() > 20
        || item.name.chars().count() > 255
        || item.file_path.chars().count() > 1024
        || item.address.chars().count() > 255
        || item.record_type.chars().count() > 32
        || item.recorder_id.chars().count() > 64
    {
        return Err(biz_error("record_query_invalid_response"));
    }
    let start_time_sec = parse_time(&item.start_time)?;
    let end_time_sec = parse_time(&item.end_time)?;
    if start_time_sec >= end_time_sec {
        return Err(biz_error("record_query_invalid_response"));
    }
    Ok(RecordSegment {
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        remote_device_id: item.device_id,
        name: item.name,
        file_path: item.file_path,
        address: item.address,
        start_time_sec,
        end_time_sec,
        secrecy: item.secrecy,
        record_type: item.record_type,
        recorder_id: item.recorder_id,
        file_size: item.file_size.max(0),
        ..Default::default()
    })
}

fn validate_request(batch_id: &str, start_time_sec: i64, end_time_sec: i64) -> GlobalResult<()> {
    if batch_id.trim().is_empty() {
        return Err(biz_error("record_query_request_id_required"));
    }
    if batch_id.chars().count() > MAX_BATCH_ID_CHARS {
        return Err(biz_error("record_query_request_id_invalid"));
    }
    if start_time_sec <= 0 || end_time_sec <= start_time_sec {
        return Err(biz_error("record_query_range_required"));
    }
    if end_time_sec.saturating_sub(start_time_sec) > MAX_RANGE_SEC {
        return Err(biz_error("record_query_range_too_large"));
    }
    Ok(())
}

fn format_time(timestamp: i64) -> GlobalResult<String> {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%dT%H:%M:%S").to_string())
        .ok_or_else(|| biz_error("record_query_range_required"))
}

fn parse_time(value: &str) -> GlobalResult<i64> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(value.timestamp());
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| biz_error("record_query_invalid_response"))?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Ok(value.timestamp()),
        LocalResult::Ambiguous(first, _) => Ok(first.timestamp()),
        LocalResult::None => Err(biz_error("record_query_invalid_response")),
    }
}

fn item_identity(item: &RecordInfoItem) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        item.device_id,
        item.start_time,
        item.end_time,
        item.file_path,
        item.record_type,
        item.recorder_id
    )
}

fn response_identity(response: &RecordInfoResponse) -> String {
    response
        .items
        .iter()
        .map(item_identity)
        .collect::<Vec<_>>()
        .join("\n")
}

fn segment_identity(item: &RecordSegment) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        item.remote_device_id,
        item.start_time_sec,
        item.end_time_sec,
        item.file_path,
        item.record_type,
        item.recorder_id
    )
}

fn biz_error(message: &str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidRequest.code(), message, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_366_day_limit() {
        assert!(validate_request("r", 1, 1 + MAX_RANGE_SEC).is_ok());
        assert!(validate_request("r", 1, 2 + MAX_RANGE_SEC).is_err());
        assert!(validate_request(&"r".repeat(MAX_BATCH_ID_CHARS + 1), 1, 2).is_err());
    }

    #[test]
    fn splits_maximum_range_into_twelve_chunks() {
        let mut count = 0;
        let mut start = 1;
        let end = start + MAX_RANGE_SEC;
        while start < end {
            start = start.saturating_add(CHUNK_RANGE_SEC).min(end);
            count += 1;
        }
        assert_eq!(count, 12);
    }

    #[test]
    fn rejects_oversized_or_inconsistent_response_totals() {
        let channel_id = "34020000001320000001";
        let mut expected = None;
        let oversized = RecordInfoResponse {
            device_id: channel_id.to_string(),
            sum_num: MAX_RESULT_ITEMS + 1,
            ..Default::default()
        };
        assert!(validate_response(channel_id, &oversized, &mut expected).is_err());

        let mut expected = None;
        let first = RecordInfoResponse {
            device_id: channel_id.to_string(),
            sum_num: 2,
            ..Default::default()
        };
        let second = RecordInfoResponse {
            device_id: channel_id.to_string(),
            sum_num: 3,
            ..Default::default()
        };
        assert!(validate_response(channel_id, &first, &mut expected).is_ok());
        assert!(validate_response(channel_id, &second, &mut expected).is_err());
    }

    #[test]
    fn rejects_record_fields_that_do_not_fit_both_database_backends() {
        let channel_id = "34020000001320000001";
        let item = RecordInfoItem {
            device_id: channel_id.to_string(),
            name: "x".repeat(256),
            start_time: "2026-07-20T01:00:00Z".to_string(),
            end_time: "2026-07-20T02:00:00Z".to_string(),
            ..Default::default()
        };
        assert!(normalize_item("34020000001110000001", channel_id, item).is_err());
    }

    #[test]
    fn removes_idle_channel_lock_without_removing_a_referenced_lock() {
        let key = "record-query-lock-cleanup-test";
        let lock = RECORD_QUERY_LOCKS
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        cleanup_query_lock(key);
        assert!(RECORD_QUERY_LOCKS.contains_key(key));
        drop(lock);
        cleanup_query_lock(key);
        assert!(!RECORD_QUERY_LOCKS.contains_key(key));
    }
}
