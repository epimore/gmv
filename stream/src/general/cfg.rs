use common::cfg_lib::conf;
use common::constructor::Get;
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::serde::Deserialize;
use common::serde_default;

#[derive(Debug, Get, Clone, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "stream", path = "config.yml")]
pub struct StreamConf {
    expires: i32,
    flv: bool,
    hls: bool,
}
serde_default!(default_expires, i32, 6);
serde_default!(default_flv, bool, true);
serde_default!(default_hls, bool, true);
impl StreamConf {
    pub fn init_by_conf() -> GlobalResult<Self> {
        let cf: StreamConf = StreamConf::conf();
        if !cf.hls && !cf.flv {
            return Err(GlobalError::new_biz_error(1200, "未启用媒体类型", |msg| error!("{msg}")));
        }
        Ok(cf)
    }
}

#[cfg(test)]
mod tests {
    use crate::general::cfg::StreamConf;

    #[test]
    fn test_init_conf() {
        let cf: StreamConf = StreamConf::conf();
        println!("{:?}", cf);
    }
}