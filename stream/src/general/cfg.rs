use common::cfg_lib::conf;
use common::cfg_lib;
use common::serde_yaml;
use common::constructor::Get;
use common::serde::Deserialize;
use common::serde_default;

#[derive(Debug, Get, Clone, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "stream")]
pub struct StreamConf {
    expires: i32,
    flv: bool,
    hls: bool,
}
serde_default!(default_expires, i32, 6);
serde_default!(default_flv, bool, true);
serde_default!(default_hls, bool, true);
impl StreamConf {
    pub fn init_by_conf() -> Self {
        StreamConf::conf()
    }
}