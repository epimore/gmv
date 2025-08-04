use common::cfg_lib::conf;
use common::constructor::Get;
use common::serde::Deserialize;
use common::serde_default;
use common::cfg_lib::conf::{CheckFromConf, FieldCheckError};

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
    pub fn init_by_conf() -> Self {
        StreamConf::conf()
    }
}

impl CheckFromConf for StreamConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if !self.hls && !self.flv {
            return Err(FieldCheckError::BizError("HLS/FLV未启用,请至少开启一个媒体输出".to_string()));
        }
        Ok(())
    }
}
#[derive(Debug, Get, Clone, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "server")]
pub struct ServerConf {
    name: String,
    rtp_port: u16,
    rtcp_port: u16,
    http_port: u16,
    hook_uri: String,
}
serde_default!(default_name, String, "stream-node-1".to_string());
serde_default!(default_rtp_port, u16, 18568);
serde_default!(default_rtcp_port, u16, 18569);
serde_default!(default_http_port, u16, 18570);
serde_default!(default_hook_uri, String, "http://127.0.0.1:18567".to_string());
impl ServerConf {
    pub fn init_by_conf() -> Self {
        ServerConf::conf()
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