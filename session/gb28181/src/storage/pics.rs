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
}

serde_default!(default_num, u8, 1);
serde_default!(default_interval, u8, 1);

impl CheckFromConf for Pics {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let uri = self.push_url.as_ref().ok_or(FieldCheckError::BizError(
            "push_url is required".to_string(),
        ))?;
        Url::parse(uri)
            .map_err(|e| FieldCheckError::BizError(format!("Invalid push_url: {}", e)))?;
        Ok(())
    }
}

impl Pics {
    pub fn get_pics_by_conf() -> &'static Self {
        static INSTANCE: Lazy<Pics> = Lazy::new(Pics::conf);
        &INSTANCE
    }
}
