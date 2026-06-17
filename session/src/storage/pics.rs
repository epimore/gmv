use std::fs;
use url::Url;

use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::once_cell::sync::Lazy;
use base::serde::Deserialize;
use base::serde_default;

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.pics", check)]
pub struct Pics {
    pub push_url: Option<String>,
    #[serde(default = "default_num")]
    pub num: u8,
    #[serde(default = "default_interval")]
    pub interval: u8,
    #[serde(default = "default_storage_path")]
    pub storage_path: String,
    #[serde(default = "default_storage_format")]
    pub storage_format: String,
}

serde_default!(default_num, u8, 1);
serde_default!(default_interval, u8, 1);
serde_default!(default_storage_path, String, "./pics/raw".to_string());
serde_default!(default_storage_format, String, "jpeg".to_string());

impl CheckFromConf for Pics {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let pics: Pics = Pics::conf();
        let uri = self.push_url.as_ref().ok_or(FieldCheckError::BizError(
            "push_url is required".to_string(),
        ))?;
        Url::parse(uri)
            .map_err(|e| FieldCheckError::BizError(format!("Invalid push_url: {}", e)))?;
        match &*pics.storage_format.to_ascii_lowercase() {
            "avif" | "bmp" | "farbfeld" | "gif" | "hdr" | "ico" | "jpeg" | "exr" | "png"
            | "pnm" | "qoi" | "tga" | "tiff" | "webp" => {}
            _ => {
                return Err(FieldCheckError::BizError("storage_format must be in [avif,bmp,farbfeld,gif,hdr,ico,jpeg,exr,png,pnm,qoi,tga,tiff,webp]".to_string()));
            }
        }
        fs::create_dir_all(&pics.storage_path).map_err(|e| {
            FieldCheckError::BizError(format!("create raw_path dir failed: {}", e.to_string()))
        })?;
        Ok(())
    }
}

impl Pics {
    pub fn get_pics_by_conf() -> &'static Self {
        static INSTANCE: Lazy<Pics> = Lazy::new(Pics::conf);
        &INSTANCE
    }
}

#[cfg(test)]
mod test {
    use base::chrono::Local;
    use image::ImageFormat;
    use image::ImageFormat::Jpeg;

    #[test]
    fn content_type_extension_maps_to_image_format() {
        let content_type = "image/jpeg";
        let format = content_type
            .split_once('/')
            .map(|(_, fmt)| fmt)
            .unwrap_or("");

        assert_eq!("jpeg", format);
        assert_eq!(Some(Jpeg), ImageFormat::from_extension(format));
        assert_eq!(8, Local::now().format("%Y%m%d").to_string().len());
    }
}
