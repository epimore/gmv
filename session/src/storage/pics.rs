use std::{fs};
use std::str::FromStr;
use cron::Schedule;
use url::Url;

use common::cfg_lib::conf;
use common::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use common::constructor::{Get};
use common::once_cell::sync::Lazy;
use common::serde::Deserialize;
use common::serde_default;

#[derive(Debug, Get, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "server.pics", check)]
pub struct Pics {
    #[serde(default = "default_enable")]
    pub enable: bool,
    pub push_url: Option<String>,
    #[serde(default = "default_cron_cycle")]
    pub cron_cycle: String,
    #[serde(default = "default_num")]
    pub num: u8,
    #[serde(default = "default_interval")]
    pub interval: u8,
    #[serde(default = "default_storage_path")]
    pub storage_path: String,
    #[serde(default = "default_storage_format")]
    pub storage_format: String,
}
serde_default!(default_enable, bool, false);
serde_default!(default_cron_cycle, String, String::from("0 */5 * * * *"));
serde_default!(default_num, u8, 1);
serde_default!(default_interval, u8, 1);
serde_default!(default_storage_path, String, "./pics/raw".to_string());
serde_default!(default_storage_format, String, "jpeg".to_string());

impl CheckFromConf for Pics {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let pics: Pics = Pics::conf();
        if pics.enable {
            let uri = self.push_url.as_ref().ok_or(FieldCheckError::BizError("push_url is required".to_string()))?;
            Url::parse(uri).map_err(|e| FieldCheckError::BizError(format!("Invalid push_url: {}", e.to_string())))?;
        }
        match &*pics.storage_format.to_ascii_lowercase() {
            "avif" | "bmp" | "farbfeld" | "gif" | "hdr" | "ico" | "jpeg" | "exr" | "png" | "pnm" | "qoi" | "tga" | "tiff" | "webp" => {}
            _ => {
                return Err(FieldCheckError::BizError("storage_format must be in [avif,bmp,farbfeld,gif,hdr,ico,jpeg,exr,png,pnm,qoi,tga,tiff,webp]".to_string()));
            }
        }
        Schedule::from_str(&pics.cron_cycle).map_err(|e| FieldCheckError::BizError(format!("Invalid cron expression: {}", e.to_string())))?;
        fs::create_dir_all(&pics.storage_path).map_err(|e| FieldCheckError::BizError(format!("create raw_path dir failed: {}", e.to_string())))?;
        Ok(())
    }
}

impl Pics {
    pub fn get_pics_by_conf() -> &'static Self {
        static INSTANCE: Lazy<Pics> = Lazy::new(|| {
            let pics: Pics = Pics::conf();
            pics
        });
        &INSTANCE
    }
}

//file_name:data
// #[derive(New)]
// pub struct ImageInfo {
//     session_id: String,
//     image_type: Option<String>,
//     file_name: String,
//     data: Vec<u8>,
// }
//
// impl ImageInfo {
//     pub fn sender() -> Sender<Self> {
//         static SENDER: Lazy<Sender<ImageInfo>> = Lazy::new(|| {
//             let (tx, rx) = crossbeam_channel::bounded(1000);
//             thread::Builder::new().name("Shared:rw".to_string()).spawn(move || {
//                 let r = rayon::ThreadPoolBuilder::new().build().expect("pics: rayon init failed");
//                 r.scope(|s| {
//                     s.spawn(move |_| {
//                         rx.iter().for_each(|image_info: ImageInfo| {
//                             let _ = image_info.hand_pic();
//                         })
//                     })
//                 })
//             }).expect("Storage:pic background thread create failed");
//             tx
//         });
//         SENDER.clone()
//     }
//
//     fn hand_pic(self) -> GlobalResult<()> {
//         let mut info = GmvFileInfo::default();
//         let now = Local::now().naive_local();
//         info.biz_time = Some(now);
//         info.create_time = Some(now);
//         info.file_type = Some(0);
//
//         let (device_id, channel_id) = se_token::split_dc(&self.session_id)?;
//         info.device_id = device_id;
//         info.channel_id = channel_id;
//         info.biz_id = self.session_id;
//         let pics_conf = Pics::get_pics_by_conf();
//         let storage_path_str = &pics_conf.storage_path;
//         let relative_path = Path::new(storage_path_str);
//         let absolute_path = fs::canonicalize(relative_path).hand_log(|msg| error!("{msg}"))?;
//         let date_str = Local::now().format("%Y%m%d").to_string();
//         let final_dir = absolute_path.join(date_str);
//         let dir_path = final_dir.to_str().ok_or_else(|| GlobalError::new_sys_error("文件存储路径错误", |msg| error!("{msg}")))?;
//         info.dir_path = dir_path.to_string();
//
//         let file_name = Path::new(&self.file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
//         info.file_name = file_name;
//         let save_path = final_dir.join(format!("{}.{}", self.file_name, pics_conf.storage_format));
//         info.file_format = Some(pics_conf.storage_format.clone());
//         let img = image::load_from_memory(&self.data).hand_log(|msg| error!("{msg}"))?;
//         img.save(&save_path).hand_log(|msg| error!("{msg}"))?;
//         let size = fs::metadata(save_path).hand_log(|msg| error!("{msg}"))?.len();
//         info.file_size = Some(size);
//         Ok(())
//     }
//
//     fn split_file_name(file_path: &PathBuf) -> (String, Option<Option<String>>) {
//         // let path = Path::new(file_name);
//         let name = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
//         let ext = file_path.extension().map(|ext| ext.to_str().map(|ext| ext.to_string()));
//         (name, ext)
//     }
// }

// fn print_diff(index: u8, last: i64) -> i64 {
//     let current = Local::now().timestamp_millis();
//     println!("{} : {}", index, current - last);
//     current
// }


#[cfg(test)]
mod test {
    use common::chrono::Local;
    use image::ImageFormat;
    use image::ImageFormat::Jpeg;
    use common::cfg_lib::conf::init_cfg;
    use crate::storage::pics::Pics;

    #[test]
    fn test() {
        let content_type = "image/jpeg";
        let format = content_type.split_once('/').map(|(_, fmt)| fmt).unwrap_or("");
        println!("格式: {}", format);
        assert_eq!("jpeg", format);
        let option = ImageFormat::from_extension(format);
        println!("{:?}", option);
        assert_eq!(Some(Jpeg), option);
        let date_str = Local::now().format("%Y%m%d").to_string();
        println!("{}", date_str);
    }

    // #[test]
    fn test_pics_conf() {
        init_cfg("config.yml".to_string());
        let conf = Pics::get_pics_by_conf();
        println!("{:?}", conf);
    }
}