use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;
use base::serde_default;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "stream", check)]
pub struct StreamConf {
    #[serde(default = "default_in_wait_timeout")]
    pub in_wait_timeout: u8,
    #[serde(default = "default_out_idle_timeout")]
    pub out_idle_timeout: u8,
}
serde_default!(default_in_wait_timeout, u8, 4);
serde_default!(default_out_idle_timeout, u8, 6);
impl StreamConf {
    pub fn init_by_conf() -> Self {
        StreamConf::conf()
    }
}
impl CheckFromConf for StreamConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.in_wait_timeout < 1 {
            return Err(FieldCheckError::BizError(
                "The in_wait_timeout must be greater than or equal to 1".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server", check)]
pub struct ServerConf {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_rtp_port")]
    pub rtp_port: u16,
    #[serde(default = "default_rtcp_port")]
    pub rtcp_port: u16,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_proxy_addr")]
    pub proxy_addr: String,
    #[serde(default)]
    pub http: HttpConf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct HttpConf {
    #[serde(default)]
    pub tls: HttpTlsConf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct HttpTlsConf {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}
serde_default!(default_name, String, "stream-node-1".to_string());
serde_default!(default_rtp_port, u16, 18568);
serde_default!(default_rtcp_port, u16, 18569);
serde_default!(default_http_port, u16, 18570);
serde_default!(
    default_host,
    String,
    env_string("GMV_STREAM_HOST", "127.0.0.1")
);
serde_default!(default_proxy_addr, String, "http:-1".to_string());

impl CheckFromConf for ServerConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.http.tls.enabled
            && (self.http.tls.certificate_path.as_os_str().is_empty()
                || self.http.tls.private_key_path.as_os_str().is_empty())
        {
            return Err(FieldCheckError::BizError(
                "server.http.tls启用时certificate_path和private_key_path不能为空".to_string(),
            ));
        }
        if self.proxy_addr != default_proxy_addr()
            && !(self.proxy_addr.starts_with("http://") || self.proxy_addr.starts_with("https://"))
        {
            return Err(FieldCheckError::BizError(
                "server.proxy_addr必须是http或https地址".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.grpc", check)]
pub struct GrpcConf {
    #[serde(default = "default_grpc_addr")]
    pub addr: SocketAddr,
    #[serde(default)]
    pub tls: GrpcTlsConf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct GrpcTlsConf {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}
serde_default!(
    default_grpc_addr,
    SocketAddr,
    env_socket_addr(
        "GMV_STREAM_CONTROL_GRPC_ADDR",
        "GMV_STREAM_CONTROL_GRPC_PORT",
        "127.0.0.1:19082"
    )
);
impl GrpcConf {
    pub fn init_by_conf() -> Self {
        GrpcConf::conf()
    }
}
impl CheckFromConf for GrpcConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.addr.port() == 0 {
            return Err(FieldCheckError::BizError(
                "server.grpc.addr端口不能为0".to_string(),
            ));
        }
        if self.tls.enabled
            && (self.tls.certificate_path.as_os_str().is_empty()
                || self.tls.private_key_path.as_os_str().is_empty())
        {
            return Err(FieldCheckError::BizError(
                "server.grpc.tls启用时certificate_path和private_key_path不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard", check)]
pub struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    pub endpoint: String,
}
serde_default!(
    default_guard_endpoint,
    String,
    env_string("GMV_GUARD_ENDPOINT", "http://127.0.0.1:18080")
);
impl GuardConf {
    pub fn init_by_conf() -> Self {
        GuardConf::conf()
    }
}
impl CheckFromConf for GuardConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.endpoint.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "guard.endpoint不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

fn env_string(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_socket_addr(addr_env: &str, port_env: &str, default: &str) -> SocketAddr {
    if let Ok(value) = std::env::var(addr_env)
        && let Ok(addr) = value.parse()
    {
        return addr;
    }
    if let Ok(value) = std::env::var(port_env)
        && let Ok(port) = value.parse::<u16>()
        && let Ok(addr) = format!("127.0.0.1:{port}").parse()
    {
        return addr;
    }
    default
        .parse()
        .expect("default socket address must be valid")
}

impl ServerConf {
    pub fn init_by_conf() -> Self {
        let mut server_conf = ServerConf::conf();
        if server_conf.proxy_addr.eq("http:-1") {
            let scheme = if server_conf.http.tls.enabled {
                "https"
            } else {
                "http"
            };
            server_conf.proxy_addr = format!(
                "{scheme}://127.0.0.1:{}/{}",
                server_conf.http_port, server_conf.name
            );
        } else {
            server_conf.proxy_addr = server_conf.proxy_addr.trim_end_matches('/').to_string();
        }
        server_conf
    }
}

#[cfg(test)]
mod tests {
    use crate::general::cfg::ServerConf;
    use base::cfg_lib::conf::init_cfg;

    //   hls 与 flv: 都为false时，触发panic
    #[test]
    fn test_check_init_conf() {
        init_cfg("config.yml".to_string());
        let cf: ServerConf = ServerConf::init_by_conf();
        println!("{:?}", cf);
    }
}
