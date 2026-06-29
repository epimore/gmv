use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::gb::sip::command as sip_command;
use crate::service::{KEY_SNAPSHOT_IMAGE, SNAPSHOT_IDLE_EXPIRES};
use crate::state;
use crate::state::model::SnapshotImage;
use crate::storage::entity::GmvFileInfo;
use crate::storage::pics::Pics;
use crate::utils::edge_token;
use axum::body::Bytes;
use base::chrono::Local;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::fs;
use base::tokio::sync::mpsc;
use base::tokio::time::Instant;

pub async fn snapshot_image(info: SnapshotImage) -> GlobalResult<String> {
    let pics_conf = Pics::get_pics_by_conf();
    let (token, session_id) = edge_token::build_token_session_id(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
    )?;
    let url = format!("{}/{}", pics_conf.push_url.trim_end_matches('/'), token);
    let count = info.count.unwrap_or(pics_conf.num);
    if count == 0 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "snapshot count must be greater than zero",
            |msg| error!("{msg}"),
        ));
    }
    let interval = info.interval.unwrap_or(pics_conf.interval);
    let (tx, mut rx) = mpsc::channel(8);
    let timeout = snapshot_idle_timeout();
    let when = Instant::now() + timeout;
    let key = rebuild_snapshot_wait_key(&session_id);
    let token_key = rebuild_pic_token(&token);
    state::session::Cache::insert_snapshot_wait(key.clone(), when, tx);
    state::session::Cache::insert_counter(token_key.clone(), count, timeout);

    if let Err(err) = sip_command::snapshot_image_call(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
        count,
        interval,
        &url,
        &session_id,
    )
    .await
    {
        state::session::Cache::remove_state(&key);
        state::session::Cache::remove_state(&token_key);
        return Err(err);
    }

    if let Some(true) = rx.recv().await {
        state::session::Cache::remove_state(&key);
        return Ok(session_id);
    }

    state::session::Cache::remove_state(&token_key);
    Err(GlobalError::new_biz_error(
        BaseErrorCode::Timeout.code(),
        "快照失败:设备不支持或响应超时",
        |msg| error!("{msg}"),
    ))
}

pub async fn upload(
    bytes: Bytes,
    content_type: &str,
    session_id: &str,
    file_id_opt: Option<&str>,
) -> GlobalResult<()> {
    let pics_conf = Pics::get_pics_by_conf();
    if bytes.is_empty() || bytes.len() > pics_conf.max_upload_bytes {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            &format!(
                "picture upload size must be 1..{} bytes",
                pics_conf.max_upload_bytes
            ),
            |msg| error!("{msg}"),
        ));
    }
    let file_format = image_extension(content_type)?;
    let (device_id, channel_id) = edge_token::split_dc(session_id)?;
    let file_name = file_id_opt
        .map(safe_file_stem)
        .filter(|value| !value.is_empty())
        .unwrap_or(edge_token::build_file_name(&device_id, &channel_id)?);
    let now = Local::now().naive_local();
    let date_str = Local::now().format("%Y%m%d").to_string();
    let final_dir = pics_conf.storage_path.join(date_str);
    fs::create_dir_all(&final_dir)
        .await
        .hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    let file_path = final_dir.join(format!("{file_name}.{file_format}"));
    fs::write(&file_path, &bytes)
        .await
        .hand_log(|msg| error!("write picture failed: {msg}"))?;
    let abs_dir = absolute_parent(&file_path).await?;
    let dir_path = final_dir.to_str().ok_or_else(|| {
        GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}"))
    })?;

    GmvFileInfo::insert_gmv_file_info(vec![GmvFileInfo {
        device_id,
        channel_id,
        biz_time: Some(now),
        biz_id: session_id.to_string(),
        file_type: Some(0),
        file_size: Some(bytes.len() as i64),
        file_name,
        file_format: Some(file_format),
        dir_path: dir_path.to_string(),
        abs_path: Some(abs_dir.to_string_lossy().to_string()),
        note: None,
        is_del: Some(0),
        create_time: Some(now),
    }])
    .await?;
    Ok(())
}

pub fn check_pic_token(token: &str) -> bool {
    state::session::Cache::decrement_counter(rebuild_pic_token(token))
}

pub fn refresh_pic_upload(token: &str, session_id: &str) {
    let timeout = snapshot_idle_timeout();
    let _ = state::session::Cache::refresh_state(&rebuild_snapshot_wait_key(session_id), timeout);
    let _ = state::session::Cache::refresh_state(&rebuild_pic_token(token), timeout);
}

fn image_extension(content_type: &str) -> GlobalResult<String> {
    let value = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "image/jpeg" | "image/jpg" => Ok("jpeg".to_string()),
        "image/png" => Ok("png".to_string()),
        "image/gif" => Ok("gif".to_string()),
        "image/webp" => Ok("webp".to_string()),
        "image/bmp" => Ok("bmp".to_string()),
        _ => Err(GlobalError::new_biz_error(
            BaseErrorCode::Unsupported.code(),
            &format!("unsupported picture content-type {content_type}"),
            |msg| error!("{msg}"),
        )),
    }
}

fn safe_file_stem(value: &str) -> String {
    Path::new(value)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(96)
        .collect()
}

async fn absolute_parent(path: &PathBuf) -> GlobalResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}"))
    })?;
    fs::canonicalize(parent)
        .await
        .hand_log(|msg| error!("resolve picture path failed: {msg}"))
}

fn rebuild_pic_token(token: &str) -> String {
    format!("SNAPSHOT:{}", token)
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
