/*
存储：
1.原始图片存储
2.生成缩略图存储
3.持久化2/3地址索引到数据库建立设备时间关系
*/

use std::fs;
use std::path::Path;
use poem::web::Field;
use uuid::Uuid;
use common::chrono::Local;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::error;
use crate::storage::entity::GmvFileInfo;
use crate::storage::pics::{Pics};
use crate::utils::se_token;


//Field { name: "upload", file_name: "天府新区.jpg", content_type: "image/jpeg" }
pub async fn upload(field: Field, session_id: String, file_id: Option<String>, snap_shot_file_id: Option<String>) -> GlobalResult<()> {
    //todo 持久化到db建立图片设备时间关系
    let file_name = if let Some(f_id) = file_id {
        f_id
    } else if let Some(ssf_if) = snap_shot_file_id {
        ssf_if
    } else if let Some(ff_name) = field.file_name() {
        ff_name.to_string()
    } else {
        Uuid::new_v4().as_simple().to_string()
    };

    let mut info = GmvFileInfo::default();
    let now = Local::now().naive_local();
    info.biz_time = Some(now);
    info.create_time = Some(now);
    info.file_type = Some(0);

    let (device_id, channel_id) = se_token::split_dc(&session_id)?;
    info.device_id = device_id;
    info.channel_id = channel_id;
    info.biz_id = session_id;
    let pics_conf = Pics::get_pics_by_conf();
    let storage_path_str = &pics_conf.storage_path;
    let relative_path = Path::new(storage_path_str);
    let absolute_path = fs::canonicalize(relative_path).hand_log(|msg| error!("{msg}"))?;
    let date_str = Local::now().format("%Y%m%d").to_string();
    let final_dir = absolute_path.join(date_str);
    let dir_path = final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?;
    info.dir_path = dir_path.to_string();

    let file_name = Path::new(&file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let save_path = final_dir.join(format!("{}.{}", file_name, pics_conf.storage_format));
    info.file_name = file_name;
    info.file_format = Some(pics_conf.storage_format.clone());
    let data = field.bytes().await.hand_log(|msg| error!("{msg}"))?;
    let img = image::load_from_memory(&data).hand_log(|msg| error!("{msg}"))?;
    img.save(&save_path).hand_log(|msg| error!("{msg}"))?;
    let size = fs::metadata(save_path).hand_log(|msg| error!("{msg}"))?.len();
    info.file_size = Some(size);
    GmvFileInfo::insert_gmv_file_info(vec![info]).await?;
    Ok(())
}

#[test]
fn t1() {
    let uuid = Uuid::new_v4().as_simple().to_string();
    println!("{}", uuid);
}