use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use base::chrono::Local;
use base::utils::{crypto, dig62};
use uuid::Uuid;

use crate::app_config::PictureUploadConfig;
use crate::core::{GuardError, GuardResult};
use crate::store::model::MediaFileInsert;
use crate::store::persistent::MediaRepository;

const SNAPSHOT_TOKEN_KEY: &str = "GMV:SESSION v1.0";
static FILE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub struct PictureUploadResult {
    pub session_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub file_name: String,
    pub file_format: String,
    pub file_size: u64,
    pub path: String,
}

pub async fn save_picture_upload(
    config: &PictureUploadConfig,
    repository: &MediaRepository,
    token: &str,
    session_id: &str,
    file_id: Option<&str>,
    content_type: &str,
    bytes: Bytes,
) -> GuardResult<PictureUploadResult> {
    verify_snapshot_token(session_id, token, config.max_session_age_sec)?;
    if bytes.is_empty() || bytes.len() > config.max_upload_bytes {
        return Err(GuardError::InvalidConfig(format!(
            "picture upload size must be 1..{} bytes",
            config.max_upload_bytes
        )));
    }
    let file_format = image_extension(content_type)?;
    let (device_id, channel_id) = split_device_channel(session_id)?;
    let file_name = file_id
        .map(safe_file_stem)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| Uuid::now_v7().simple().to_string());
    let date_dir = Local::now().format("%Y%m%d").to_string();
    let final_dir = config.storage_path.join(&date_dir);
    base::tokio::fs::create_dir_all(&final_dir)
        .await
        .map_err(|error| {
            GuardError::Conflict(format!("create picture directory failed: {error}"))
        })?;
    let file_path = final_dir.join(format!("{file_name}.{file_format}"));
    base::tokio::fs::write(&file_path, &bytes)
        .await
        .map_err(|error| GuardError::Conflict(format!("write picture failed: {error}")))?;
    let abs_dir = absolute_parent(&file_path)?;
    let dir_path = final_dir.to_string_lossy().to_string();
    let now = Local::now()
        .naive_local()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let file_size = bytes.len() as u64;
    repository
        .insert_media_file(&MediaFileInsert {
            id: next_file_id(),
            device_id: device_id.clone(),
            channel_id: channel_id.clone(),
            biz_time: now.clone(),
            biz_id: session_id.to_string(),
            file_type: 0,
            file_size,
            file_name: file_name.clone(),
            file_format: Some(file_format.clone()),
            dir_path,
            abs_path: Some(abs_dir.to_string_lossy().to_string()),
            note: None,
            is_del: 0,
            create_time: now,
        })
        .await?;
    Ok(PictureUploadResult {
        session_id: session_id.to_string(),
        device_id,
        channel_id,
        file_name,
        file_format,
        file_size,
        path: file_path.to_string_lossy().to_string(),
    })
}

pub(crate) fn next_file_id() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128 / 1000) as i64
        });
    let seq = FILE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed) % 1000;
    millis.saturating_mul(1000).saturating_add(seq as i64)
}

fn verify_snapshot_token(session_id: &str, token: &str, max_age_sec: u64) -> GuardResult<()> {
    let input = format!("{SNAPSHOT_TOKEN_KEY}@{session_id}");
    let expected = crypto::generate_token(&input);
    if expected != token {
        return Err(GuardError::InvalidIdentity(
            "invalid picture upload token".to_string(),
        ));
    }
    let decoded = dig62::de(session_id)
        .map_err(|error| GuardError::InvalidConfig(format!("invalid session id: {error}")))?;
    if decoded.len() < 41 {
        return Err(GuardError::InvalidConfig("invalid session id".to_string()));
    }
    let millis = decoded[40..]
        .parse::<u128>()
        .map_err(|_| GuardError::InvalidConfig("invalid session timestamp".to_string()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GuardError::TimeUnsynced("system time went backwards".to_string()))?
        .as_millis();
    if now.saturating_sub(millis) > u128::from(max_age_sec) * 1000 {
        return Err(GuardError::InvalidIdentity(
            "picture upload token expired".to_string(),
        ));
    }
    Ok(())
}

fn split_device_channel(session_id: &str) -> GuardResult<(String, String)> {
    let decoded = dig62::de(session_id)
        .map_err(|error| GuardError::InvalidConfig(format!("invalid session id: {error}")))?;
    if decoded.len() < 40 {
        return Err(GuardError::InvalidConfig("invalid session id".to_string()));
    }
    Ok((decoded[0..20].to_string(), decoded[20..40].to_string()))
}

fn image_extension(content_type: &str) -> GuardResult<String> {
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
        _ => Err(GuardError::InvalidConfig(format!(
            "unsupported picture content-type {content_type}"
        ))),
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

fn absolute_parent(path: &PathBuf) -> GuardResult<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        GuardError::Conflict("picture path parent directory is missing".to_string())
    })?;
    parent
        .canonicalize()
        .map_err(|error| GuardError::Conflict(format!("resolve picture path failed: {error}")))
}
