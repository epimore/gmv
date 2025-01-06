/*
存储：
1.原始图片存储
2.生成缩略图存储
3.持久化2/3地址索引到数据库建立设备时间关系
*/

use poem::web::Field;
use crate::storage::pics::{ImageInfo};

// pub struct UploadInfo {
//     load: Vec<u8>,
//     uk: String,
//     file_id: String,
//     content_type: String,
// }

//Field { name: "upload", file_name: "天府新区.jpg", content_type: "image/jpeg" }
pub async fn upload(field: Field, _uk: String, _session_id: Option<String>,file_id: Option<String>, snap_shot_file_id: Option<String>) {
    //todo 验证uk
    //todo 持久化到db建立图片设备时间关系
    let file_name = if let Some(f_id) = file_id {
        f_id
    } else if let Some(ssf_if) = snap_shot_file_id {
        ssf_if
    } else if let Some(ff_name) = field.file_name() {
        ff_name.to_string()
    }else {
        "session_id".to_string()
    };
    let file_type = field.content_type().unwrap().to_string();
    let data = field.bytes().await.unwrap();
    let image_info = ImageInfo::new(file_type, file_name, data);
    let tx = ImageInfo::sender();
    tx.send(image_info).unwrap();
}

// pub async fn snapshot_image(){
//     let _conf = Pics::get_pics_by_conf();
//     unimplemented!()
// }