use std::path::PathBuf;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;
use base::serde_default;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.pics", check)]
pub struct Pics {
    #[serde(default = "default_push_url")]
    pub push_url: String,
    #[serde(default = "default_storage_path")]
    pub storage_path: PathBuf,
    #[serde(default = "default_storage_format")]
    pub storage_format: String,
    #[serde(default = "default_num")]
    pub num: u8,
    #[serde(default = "default_interval")]
    pub interval: u8,
    #[serde(default = "default_max_upload_bytes")]
    pub max_upload_bytes: usize,
}

serde_default!(
    default_push_url,
    String,
    "http://127.0.0.1:18567/edge/upload/picture".to_string()
);
serde_default!(default_storage_path, PathBuf, PathBuf::from("./pics/raw"));
serde_default!(default_storage_format, String, "jpeg".to_string());
serde_default!(default_num, u8, 1);
serde_default!(default_interval, u8, 1);
serde_default!(default_max_upload_bytes, usize, 10 * 1024 * 1024);

impl Pics {
    pub fn get_pics_by_conf() -> Self {
        Self::conf()
    }
}

impl CheckFromConf for Pics {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if !(self.push_url.starts_with("http://") || self.push_url.starts_with("https://")) {
            return Err(FieldCheckError::BizError(
                "server.pics.push_url必须是http或https地址".to_string(),
            ));
        }
        if self.storage_path.as_os_str().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.pics.storage_path不能为空".to_string(),
            ));
        }
        if self.storage_format.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "server.pics.storage_format不能为空".to_string(),
            ));
        }
        if self.num == 0 || self.interval == 0 || self.max_upload_bytes == 0 {
            return Err(FieldCheckError::BizError(
                "server.pics配置中的数量、间隔和大小限制必须大于0".to_string(),
            ));
        }
        Ok(())
    }
}
