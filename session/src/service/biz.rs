/*
存储：
1.原始图片存储
2.生成缩略图存储
3.持久化2/3地址索引到数据库建立设备时间关系
*/

use std::fs;
use std::path::{Path, PathBuf};
use poem::Body;
use poem_openapi::payload::Binary;
use common::chrono::Local;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::error;
use common::tokio::io::AsyncReadExt;
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::storage::pics::{Pics};
use crate::utils::se_token;

pub async fn upload(data: Binary<Body>, session_id: String, file_id: Option<String>, snap_shot_file_id: Option<String>) -> GlobalResult<()> {
    let id = snap_shot_file_id.or(file_id);
    let (device_id, channel_id) = se_token::split_dc(&session_id)?;
    let file_name = match id {
        None => {
            se_token::build_file_name(&device_id, &channel_id)?
        }
        Some(id) => {
            id
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
    info.biz_id = session_id;
    let pics_conf = Pics::get_pics_by_conf();
    let storage_path_str = &pics_conf.storage_path;
    let relative_path = Path::new(storage_path_str);
    let date_str = Local::now().format("%Y%m%d").to_string();
    let final_dir = relative_path.join(date_str);
    fs::create_dir_all(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    let abs_final_dir = std::fs::canonicalize(&final_dir).hand_log(|msg| error!("create pics dir failed: {msg}"))?;
    info.abs_path = abs_final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?.to_string();
    let dir_path = final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?;
    info.dir_path = dir_path.to_string();

    let file_name = Path::new(&file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let save_path = final_dir.join(format!("{}.{}", file_name, pics_conf.storage_format.to_ascii_lowercase()));
    info.file_name = file_name;
    info.file_format = Some(pics_conf.storage_format.to_ascii_lowercase());

    let mut reader = data.0.into_async_read();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.hand_log(|msg| error!("read pics bytes failed: {msg}"))?;
    let img = image::load_from_memory(&bytes).hand_log(|msg| error!("{msg}"))?;
    img.save(&save_path).hand_log(|msg| error!("{msg}"))?;
    let size = fs::metadata(save_path).hand_log(|msg| error!("{msg}"))?.len();
    info.file_size = Some(size);
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
        //    use common::exception::{GlobalError, GlobalResult};
        //     use common::log::error;
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