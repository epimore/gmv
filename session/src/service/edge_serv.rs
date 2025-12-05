/*
存储：
1.原始图片存储
2.生成缩略图存储
3.持久化2/3地址索引到数据库建立设备时间关系
*/
use crate::gb::depot::extract::HeaderItemExt;
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::storage::pics::Pics;
use crate::utils::edge_token;
use base::bytes::Bytes;
use base::chrono::Local;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use base::serde_json;
use base::tokio::sync::mpsc;
use base::tokio::time::Instant;
use rsip::StatusCodeKind;
use shared::info::obj::BaseStreamInfo;
use crate::gb::handler::cmd;
use crate::service::{EXPIRES, KEY_SNAPSHOT_IMAGE, KEY_STREAM_IN};
use crate::state;
use crate::state::model::SnapshotImage;
use crate::utils::edge_token::build_session_id;

pub async fn snapshot_image(info:SnapshotImage) ->GlobalResult<String>{
    let pics_conf = Pics::get_pics_by_conf();
    let (token, session_id) = edge_token::build_token_session_id(&info.device_channel_ident.device_id, &info.device_channel_ident.channel_id)?;
    let url = format!("{}/{}", pics_conf.push_url.clone().unwrap(), token);
    let count = info.count.unwrap_or_else(|| pics_conf.num);
    let response = cmd::CmdControl::snapshot_image_call(&info.device_channel_ident.device_id, &info.device_channel_ident.channel_id, count, pics_conf.interval, &url, &session_id).await?;
    if !matches!(response.status_code.kind(),StatusCodeKind::Successful)  {
        Err(GlobalError::new_sys_error("snapshot image failed", |msg| error!("{msg}")))?
    };
    let (tx, mut rx) = mpsc::channel(8);
    let when = Instant::now() + Duration::from_secs(EXPIRES);
    let key = format!("{}{}", KEY_SNAPSHOT_IMAGE,session_id);
    state::session::Cache::state_insert(key.clone(), Bytes::new(), Some(when), Some(tx));
    if let Some(Some(_)) = rx.recv().await {
        state::session::Cache::state_remove(&key);
        return Ok(session_id)
    }
    Err(GlobalError::new_biz_error(1100,"快照失败:设备不支持或响应超时", |msg| error!("{msg}")))?
}

pub async fn upload(bytes: Bytes, session_id: &str, file_id_opt: Option<&String>) -> GlobalResult<()> {
    let (device_id, channel_id) = edge_token::split_dc(session_id)?;
    let file_name = match file_id_opt {
        None => {
            edge_token::build_file_name(&device_id, &channel_id)?
        }
        Some(id) => {
            id.to_string()
        }
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
    let storage_path_str = &pics_conf.storage_path;
    let relative_path = Path::new(storage_path_str);
    let date_str = Local::now().format("%Y%m%d").to_string();
    let final_dir = relative_path.join(date_str);
    fs::create_dir_all(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    let abs_final_dir = fs::canonicalize(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    info.abs_path = abs_final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?.to_string();
    let dir_path = final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?;
    info.dir_path = dir_path.to_string();

    let file_name = Path::new(&file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let save_path = final_dir.join(format!("{}.{}", file_name, pics_conf.storage_format.to_ascii_lowercase()));
    info.file_name = file_name;
    info.file_format = Some(pics_conf.storage_format.to_ascii_lowercase());

    // let mut reader = data.0.into_async_read();
    // let mut bytes = Vec::new();
    // reader.read_to_end(&mut bytes).await.hand_log(|msg| error!("read pics bytes failed: {msg}"))?;
    let img = image::load_from_memory(&bytes).hand_log(|msg| error!("{msg}"))?;
    img.save(&save_path).hand_log(|msg| error!("{msg}"))?;
    let size = fs::metadata(save_path).hand_log(|msg| error!("{msg}"))?.len();
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

mod test {
    #[test]
    fn test_path() {
        //    use base::exception::{GlobalError, GlobalResult};
        //     use base::log::error;
        //     use crate::general::DownloadConf;
        // use std::{env, fs};
        // use std::path::{Path, PathBuf};
        // let relative_path = Path::new("./storage/pics/");
        // let final_dir = relative_path.join("2025");
        // let abs_final_dir = env::current_dir().unwrap().join(&final_dir);
        // println!("{:?}", abs_final_dir);
        // fs::create_dir_all(&final_dir).unwrap();
        // let abs_final_dir = std::fs::canonicalize(&final_dir).unwrap();
        // println!("{}", abs_final_dir.to_str().unwrap());


        // fn get_path(path_file_name: &String) -> GlobalResult<(String, String, String, String)> {
        //     let path = Path::new(&path_file_name);
        //     let dir_path = DownloadConf::get_download_conf().storage_path;
        //     let biz_id = path.file_stem().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
        //     let extension = path.extension().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
        //     let abs_path = path.parent().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
        //     Ok((abs_path, dir_path, biz_id, extension))
        // }
    }
}