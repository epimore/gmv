use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base::bytes::Bytes;
use base::chrono::Local;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::mpsc;
use base::tokio::time::Instant;
use rsip::StatusCodeKind;

use crate::gb::handler::cmd;
use crate::service::{EXPIRES, KEY_SNAPSHOT_IMAGE};
use crate::state;
use crate::state::model::SnapshotImage;
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::storage::pics::Pics;
use crate::utils::edge_token;

pub async fn snapshot_image(info: SnapshotImage) -> GlobalResult<String> {
    let pics_conf = Pics::get_pics_by_conf();
    let (token, session_id) = edge_token::build_token_session_id(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
    )?;
    let url = format!("{}/{}", pics_conf.push_url.clone().unwrap(), token);
    let count = info.count.unwrap_or_else(|| pics_conf.num);
    let response = cmd::CmdControl::snapshot_image_call(
        &info.device_channel_ident.device_id,
        &info.device_channel_ident.channel_id,
        count,
        pics_conf.interval,
        &url,
        &session_id,
    )
    .await?;
    if !matches!(response.status_code.kind(), StatusCodeKind::Successful) {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "snapshot image failed",
            |msg| error!("{msg}"),
        ))?
    };

    let (tx, mut rx) = mpsc::channel(8);
    let when = Instant::now() + Duration::from_secs(EXPIRES);
    let key = format!("{}{}", KEY_SNAPSHOT_IMAGE, session_id);
    state::session::Cache::insert_snapshot_wait(key.clone(), when, tx);
    cache_pic_token(token, count);

    if let Some(true) = rx.recv().await {
        state::session::Cache::remove_state(&key);
        return Ok(session_id);
    }

    Err(GlobalError::new_biz_error(
        BaseErrorCode::Timeout.code(),
        "快照失败:设备不支持或响应超时",
        |msg| error!("{msg}"),
    ))?
}

pub async fn upload(
    bytes: Bytes,
    session_id: &str,
    file_id_opt: Option<&String>,
) -> GlobalResult<()> {
    let (device_id, channel_id) = edge_token::split_dc(session_id)?;
    let file_name = match file_id_opt {
        None => edge_token::build_file_name(&device_id, &channel_id)?,
        Some(id) => id.to_string(),
    };

    let mut info = GmvFileInfo::default();
    let now = Local::now().naive_local();
    info.biz_time = Some(now);
    info.create_time = Some(now);
    info.file_type = Some(0);
    info.is_del = Some(0);
    info.device_id = device_id;
    info.channel_id = channel_id;
    info.biz_id = session_id.to_string();

    let pics_conf = Pics::get_pics_by_conf();
    let relative_path = Path::new(&pics_conf.storage_path);
    let date_str = Local::now().format("%Y%m%d").to_string();
    let final_dir = relative_path.join(date_str);
    fs::create_dir_all(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    let abs_final_dir =
        fs::canonicalize(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    info.abs_path = abs_final_dir
        .to_str()
        .ok_or_else(|| {
            GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}"))
        })?
        .to_string();
    let dir_path = final_dir.to_str().ok_or_else(|| {
        GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}"))
    })?;
    info.dir_path = dir_path.to_string();

    let file_name = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let save_path = final_dir.join(format!(
        "{}.{}",
        file_name,
        pics_conf.storage_format.to_ascii_lowercase()
    ));
    info.file_name = file_name;
    info.file_format = Some(pics_conf.storage_format.to_ascii_lowercase());

    let img = image::load_from_memory(&bytes).hand_log(|msg| error!("{msg}"))?;
    img.save(&save_path).hand_log(|msg| error!("{msg}"))?;
    let size = fs::metadata(save_path)
        .hand_log(|msg| error!("{msg}"))?
        .len();
    info.file_size = size;
    GmvFileInfo::insert_gmv_file_info(vec![info]).await?;
    Ok(())
}

pub async fn rm_file(file_id: i64) -> GlobalResult<()> {
    if let Ok(file_info) = GmvFileInfo::query_gmv_file_info_by_id(file_id).await {
        let mut file = file_info.file_name.clone();
        if let Some(ext) = &file_info.file_format {
            file = format!("{}.{}", file, ext);
        }
        let path_buf = PathBuf::from(&file_info.dir_path).join(file);
        if path_buf.exists() {
            fs::remove_file(path_buf).hand_log(|msg| error!("{msg}"))?;
            GmvFileInfo::rm_gmv_file_info_by_id(file_id).await?;
            GmvRecord::rm_gmv_record_by_biz_id(&file_info.biz_id).await?;
        }
    }
    Ok(())
}

pub fn cache_pic_token(token: String, num: u8) {
    state::session::Cache::insert_counter(rebuild_pic_token(token), num, Duration::from_secs(300));
}

pub fn check_pic_token(token: String) -> bool {
    state::session::Cache::decrement_counter(rebuild_pic_token(token))
}

fn rebuild_pic_token(token: String) -> String {
    format!("SNAPSHOT:{}", token)
}

mod test {
    #[test]
    fn test_path() {}
}
