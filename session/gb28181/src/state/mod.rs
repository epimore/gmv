use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;
use base::serde_default;
use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Mutex, OnceLock};

pub mod model;
pub mod session;

#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.grpc", check)]
pub struct SessionGrpcConf {
    #[serde(default = "default_session_grpc_addr")]
    pub addr: SocketAddr,
}

serde_default!(
    default_session_grpc_addr,
    SocketAddr,
    env_socket_addr(
        "GMV_SESSION_CONTROL_GRPC_ADDR",
        "GMV_SESSION_CONTROL_GRPC_PORT",
        "127.0.0.1:19081"
    )
);

impl CheckFromConf for SessionGrpcConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.addr.port() == 0 {
            return Err(FieldCheckError::BizError(
                "server.grpc.addr端口不能为0".to_string(),
            ));
        }
        Ok(())
    }
}

impl SessionGrpcConf {
    pub fn get() -> Self {
        Self::conf()
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
    #[serde(default = "default_guard_http_endpoint")]
    pub http_endpoint: String,
}

serde_default!(
    default_guard_endpoint,
    String,
    "http://127.0.0.1:18080".to_string()
);
serde_default!(
    default_guard_http_endpoint,
    String,
    "http://127.0.0.1:8080".to_string()
);

impl CheckFromConf for GuardConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.endpoint.trim().is_empty() || self.http_endpoint.trim().is_empty() {
            return Err(FieldCheckError::BizError(
                "guard.endpoint和guard.http_endpoint不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for GuardConf {
    fn default() -> Self {
        Self {
            endpoint: default_guard_endpoint(),
            http_endpoint: default_guard_http_endpoint(),
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

    pub fn picture_upload_url(&self) -> String {
        format!(
            "{}/api/v2/edge/upload/picture",
            self.http_endpoint.trim_end_matches('/')
        )
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
    pub local_ip: Ipv4Addr,
    pub local_port: u16,
    pub control_grpc_uri: String,
    pub pub_ip: Ipv4Addr,
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
}
impl CheckFromConf for DownloadConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let dc = DownloadConf::conf();
        fs::create_dir_all(&dc.storage_path).map_err(|e| {
            FieldCheckError::BizError(format!("create download dir failed: {}", e.to_string()))
        })?;
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
