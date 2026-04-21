use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::constructor::Get;
use base::serde::Deserialize;
use base::serde_default;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "stream",check)]
pub struct StreamConf {
    pub in_wait_timeout: u8,
    pub out_idle_timeout: u8,
}
serde_default!(default_in_wait_timeout, i32, 4);
serde_default!(default_out_idle_timeout, i32, 6);
impl StreamConf {
    pub fn init_by_conf() -> Self {
        StreamConf::conf()
    }
}
impl CheckFromConf for StreamConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.in_wait_timeout<1 {
            return Err(FieldCheckError::BizError("The in_wait_timeout must be greater than or equal to 1".to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server")]
pub struct ServerConf {
    pub name: String,
    pub rtp_port: u16,
    pub rtcp_port: u16,
    pub http_port: u16,
    pub hook_uri: String,
    pub proxy_addr: String,
}
serde_default!(default_name, String, "stream-node-1".to_string());
serde_default!(default_rtp_port, u16, 18568);
serde_default!(default_rtcp_port, u16, 18569);
serde_default!(default_http_port, u16, 18570);
serde_default!(
    default_hook_uri,
    String,
    "http://127.0.0.1:18567".to_string()
);
serde_default!(default_proxy_addr, String, "http:-1".to_string());
impl ServerConf {
    pub fn init_by_conf() -> Self {
        let mut server_conf = ServerConf::conf();
        if server_conf.proxy_addr.eq("http:-1") {
            server_conf.proxy_addr = format!("http://127.0.0.1:{}/{}", server_conf.http_port,server_conf.name);
        }else {
            server_conf.proxy_addr = format!("{}/{}", server_conf.proxy_addr,server_conf.name);
        }
        server_conf
    }
}

#[cfg(test)]
mod tests {
    use crate::general::cfg::{ServerConf};
    use base::cfg_lib::conf::init_cfg;

    //   hls 与 flv: 都为false时，触发panic
    #[test]
    fn test_check_init_conf() {
        init_cfg("config.yml".to_string());
        let cf: ServerConf = ServerConf::init_by_conf();
        println!("{:?}", cf);
    }
}
