use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;
use base::serde_default;
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub mod model;
pub mod session;

#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.grpc", check)]
pub struct SessionGrpcConf {
    #[serde(default = "default_session_grpc_listen_addr")]
    pub listen_addr: SocketAddr,
    #[serde(default = "default_session_grpc_advertised_url")]
    pub advertised_url: String,
    #[serde(default)]
    pub tls: GrpcTlsConf,
}

#[derive(Debug, Deserialize, Clone, Default)]
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
    default_session_grpc_listen_addr,
    SocketAddr,
    env_socket_addr(
        "GMV_SESSION_CONTROL_GRPC_ADDR",
        "GMV_SESSION_CONTROL_GRPC_PORT",
        "127.0.0.1:19081"
    )
);
serde_default!(
    default_session_grpc_advertised_url,
    String,
    "http://127.0.0.1:19081".to_string()
);

impl CheckFromConf for SessionGrpcConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.listen_addr.port() == 0 {
            return Err(FieldCheckError::BizError(
                "server.grpc.listen_addr端口不能为0".to_string(),
            ));
        }
        self.advertised_endpoint()
            .map_err(FieldCheckError::BizError)?;
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

impl SessionGrpcConf {
    pub fn get() -> Self {
        Self::conf()
    }

    pub fn endpoint(&self) -> String {
        let (tls, host, port) = self
            .advertised_endpoint()
            .expect("validated session gRPC advertised URL");
        base_rpc::rpc_endpoint_uri(tls, &host, port)
    }

    pub fn advertised_endpoint(&self) -> Result<(bool, String, u16), String> {
        let url = url::Url::parse(&self.advertised_url)
            .map_err(|error| format!("server.grpc.advertised_url地址无效: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("server.grpc.advertised_url必须使用http或https".to_string());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("server.grpc.advertised_url不能包含凭据".to_string());
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err("server.grpc.advertised_url不能包含path、query或fragment".to_string());
        }
        let host = url
            .host_str()
            .filter(|host| !host.trim().is_empty())
            .ok_or_else(|| "server.grpc.advertised_url必须包含host".to_string())?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "server.grpc.advertised_url必须包含有效端口".to_string())?;
        Ok((url.scheme() == "https", host.to_string(), port))
    }
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

#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
#[conf(prefix = "guard", check)]
pub struct GuardConf {
    #[serde(default = "default_guard_endpoint")]
    pub endpoint: String,
}

serde_default!(
    default_guard_endpoint,
    String,
    "http://127.0.0.1:18080".to_string()
);

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

impl Default for GuardConf {
    fn default() -> Self {
        Self {
            endpoint: default_guard_endpoint(),
        }
    }
}

impl GuardConf {
    pub fn get() -> Self {
        Self::conf()
    }

    pub fn get_or_default() -> Self {
        std::panic::catch_unwind(Self::conf).unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.alarm", check)]
pub struct AlarmConf {
    pub enable: bool,
    #[serde(default = "default_priority")]
    pub priority: u8,
}
serde_default!(default_priority, u8, 4);
static ALARM_CONF: OnceLock<AlarmConf> = OnceLock::new();

impl AlarmConf {
    pub fn get_alarm_conf() -> &'static Self {
        ALARM_CONF.get_or_init(|| AlarmConf::conf())
    }
}

impl CheckFromConf for AlarmConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.priority == 0 || self.priority > 4 {
            return Err(FieldCheckError::BizError(
                "server.alarm.priority必须为1|2|3|4".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct StreamNode {
    pub name: String,
    pub control_grpc_uri: String,
    pub pub_host: String,
    pub pub_port: u16,
}

static STREAM_NODES: OnceLock<Mutex<HashMap<String, StreamNode>>> = OnceLock::new();

pub struct StreamNodeRegistry;

impl StreamNodeRegistry {
    fn nodes() -> &'static Mutex<HashMap<String, StreamNode>> {
        STREAM_NODES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn upsert(node: StreamNode) {
        if let Ok(mut nodes) = Self::nodes().lock() {
            nodes.insert(node.name.clone(), node);
        }
    }

    pub fn get(node_id: &str) -> Option<StreamNode> {
        Self::nodes()
            .lock()
            .ok()
            .and_then(|nodes| nodes.get(node_id).cloned())
    }

    pub fn contains(node_id: &str) -> bool {
        Self::get(node_id).is_some()
    }

    pub fn node_names() -> Vec<String> {
        Self::nodes()
            .lock()
            .map(|nodes| nodes.keys().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.download", check)]
pub struct DownloadConf {
    pub storage_path: String,
    #[serde(default = "default_storage_id")]
    pub storage_id: String,
    #[serde(default)]
    pub public_base_url: String,
    #[serde(default = "default_min_free_bytes")]
    pub min_free_bytes: u64,
    #[serde(default = "default_access_ticket_idle_ttl_secs")]
    pub access_ticket_idle_ttl_secs: u64,
    #[serde(default = "default_access_ticket_max_ttl_secs")]
    pub access_ticket_max_ttl_secs: u64,
}
serde_default!(
    default_storage_id,
    String,
    "cloud-recording-main".to_string()
);
serde_default!(default_min_free_bytes, u64, 2 * 1024 * 1024 * 1024);
serde_default!(default_access_ticket_idle_ttl_secs, u64, 300);
serde_default!(default_access_ticket_max_ttl_secs, u64, 21_600);
impl CheckFromConf for DownloadConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let dc = DownloadConf::conf();
        fs::create_dir_all(&dc.storage_path).map_err(|e| {
            FieldCheckError::BizError(format!("create download dir failed: {}", e.to_string()))
        })?;
        if dc.access_ticket_idle_ttl_secs == 0
            || dc.access_ticket_max_ttl_secs < dc.access_ticket_idle_ttl_secs
        {
            return Err(FieldCheckError::BizError(
                "server.download access ticket TTL 配置无效".to_string(),
            ));
        }
        let public_base_url = dc.public_base_url.trim();
        if !public_base_url.is_empty()
            && !(public_base_url.starts_with("http://") || public_base_url.starts_with("https://"))
        {
            return Err(FieldCheckError::BizError(
                "server.download.public_base_url必须是http或https地址".to_string(),
            ));
        }
        Ok(())
    }
}

impl DownloadConf {
    pub fn get_download_conf() -> Self {
        DownloadConf::conf()
    }
}

#[cfg(test)]
mod tests {
    use super::{GrpcTlsConf, SessionGrpcConf};
    use crate::http::Http;
    use base::cfg_lib::conf::CheckFromConf;

    #[test]
    fn example_http_and_grpc_config_deserializes() {
        let root: base::serde_yaml::Value =
            base::serde_yaml::from_str(include_str!("../../config.yml")).unwrap();
        let http: Http = base::serde_yaml::from_value(root.get("http").cloned().unwrap()).unwrap();
        let grpc: SessionGrpcConf = base::serde_yaml::from_value(
            root.get("server")
                .and_then(|server| server.get("grpc"))
                .cloned()
                .unwrap(),
        )
        .unwrap();

        http.public_endpoint().unwrap();
        grpc.advertised_endpoint().unwrap();
    }

    #[test]
    fn advertised_grpc_tls_is_independent_from_listener_tls() {
        let conf = SessionGrpcConf {
            listen_addr: "127.0.0.1:19081".parse().unwrap(),
            advertised_url: "https://session-rpc.example.com:443".to_string(),
            tls: GrpcTlsConf::default(),
        };

        assert_eq!(
            conf.advertised_endpoint().unwrap(),
            (true, "session-rpc.example.com".to_string(), 443)
        );
        assert_eq!(conf.endpoint(), "https://session-rpc.example.com:443");
        conf._field_check().unwrap();
    }

    #[test]
    fn grpc_config_ignores_extra_fields() {
        let yaml = r#"
listen_addr: 127.0.0.1:19081
advertised_url: https://session-rpc.example.com
addr: 127.0.0.1:18081
"#;
        let conf: SessionGrpcConf = base::serde_yaml::from_str(yaml).unwrap();
        assert_eq!(conf.listen_addr, "127.0.0.1:19081".parse().unwrap());
        assert_eq!(conf.advertised_url, "https://session-rpc.example.com");
    }

    fn print_banner(c: char) {
        let binary = match c {
            'G' => [0b11111, 0b10000, 0b10011, 0b10001, 0b11111],
            'M' => [0b10001, 0b11011, 0b10101, 0b10001, 0b10001],
            'V' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100],
            _ => [0; 5],
        };

        for &row in binary.iter() {
            for i in (0..5).rev() {
                print!("{}", if row & (1 << i) == 0 { ' ' } else { '#' });
            }
            println!();
        }
        println!();
    }

    #[test]
    fn test_banner() {
        print_banner('G');
        print_banner('M');
        print_banner('V');
    }
}
