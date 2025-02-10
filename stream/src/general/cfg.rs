use common::cfg_lib::conf;
use common::constructor::Get;
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::serde::Deserialize;
use common::serde_default;
use common::cfg_lib::conf::CheckFromConf;

#[derive(Debug, Get, Clone, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "stream", check)]
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
        Ok(cf)
    }
}

impl CheckFromConf for StreamConf {
    fn _field_check(&self) {
        if !self.hls && !self.flv {
            return panic!("未启用媒体类型");
        }
    }
}

#[cfg(test)]
mod tests {
    use common::cfg_lib::conf::init_cfg;
    use crate::general::cfg::StreamConf;

    //   hls 与 flv: 都为false时，触发panic
    #[test]
    fn test_check_init_conf() {
        init_cfg("config.yml".to_string());
        let cf: StreamConf = StreamConf::conf();
        println!("{:?}", cf);
    }
}