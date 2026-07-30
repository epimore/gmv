use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::serde::Deserialize;
use base::serde_default;
use std::net::{IpAddr, SocketAddr};
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
serde_default!(default_out_idle_timeout, u8, 20);
const MIN_IN_WAIT_TIMEOUT: u8 = 1;
const MAX_IN_WAIT_TIMEOUT: u8 = 30;
const MIN_OUT_IDLE_TIMEOUT: u8 = 12;
const MAX_OUT_IDLE_TIMEOUT: u8 = 120;
impl StreamConf {
    pub fn init_by_conf() -> Self {
        StreamConf::conf()
    }

    pub fn resolve_media_timeouts(
        &self,
        in_wait_timeout: Option<u8>,
        out_idle_timeout: Option<u8>,
    ) -> Result<(u8, u8), String> {
        let in_wait_timeout = in_wait_timeout.unwrap_or(self.in_wait_timeout);
        let out_idle_timeout = out_idle_timeout.unwrap_or(self.out_idle_timeout);
        validate_media_timeouts(in_wait_timeout, out_idle_timeout)?;
        Ok((in_wait_timeout, out_idle_timeout))
    }
}
impl CheckFromConf for StreamConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        validate_media_timeouts(self.in_wait_timeout, self.out_idle_timeout)
            .map_err(FieldCheckError::BizError)
    }
}

fn validate_media_timeouts(in_wait_timeout: u8, out_idle_timeout: u8) -> Result<(), String> {
    if !(MIN_IN_WAIT_TIMEOUT..=MAX_IN_WAIT_TIMEOUT).contains(&in_wait_timeout) {
        return Err(format!(
            "stream.in_wait_timeout must be between {MIN_IN_WAIT_TIMEOUT} and {MAX_IN_WAIT_TIMEOUT} seconds"
        ));
    }
    if !(MIN_OUT_IDLE_TIMEOUT..=MAX_OUT_IDLE_TIMEOUT).contains(&out_idle_timeout) {
        return Err(format!(
            "stream.out_idle_timeout must be between {MIN_OUT_IDLE_TIMEOUT} and {MAX_OUT_IDLE_TIMEOUT} seconds"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
#[conf(prefix = "server", check)]
pub struct ServerConf {
    #[serde(default = "default_name")]
    pub name: String,
    pub http: HttpServerConf,
    pub grpc: GrpcServerConf,
    pub media: MediaServerConf,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MediaListenerMode {
    Single,
    Multi,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct MediaPortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    crate = "base::serde",
    tag = "mode",
    rename_all = "lowercase",
    deny_unknown_fields
)]
pub enum MediaServerConf {
    Single {
        listen_ip: IpAddr,
        advertised_host: String,
        port: u16,
    },
    Multi {
        listen_ip: IpAddr,
        advertised_host: String,
        port_range: MediaPortRange,
        reservation_timeout_secs: u64,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MediaListenerConf {
    pub mode: MediaListenerMode,
    pub bind_ip: IpAddr,
    pub advertised_host: String,
    pub single_port: u16,
    pub port_range: MediaPortRange,
    pub reservation_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct HttpServerConf {
    pub listen_addr: SocketAddr,
    pub public_url: String,
    #[serde(default)]
    pub tls: HttpTlsConf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct HttpTlsConf {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}
serde_default!(default_name, String, "stream-node-1".to_string());
#[derive(Debug, Clone, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct GrpcServerConf {
    pub listen_addr: SocketAddr,
    pub advertised_addr: SocketAddr,
    #[serde(default)]
    pub tls: GrpcTlsConf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(crate = "base::serde", deny_unknown_fields)]
pub struct GrpcTlsConf {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}

impl HttpServerConf {
    pub fn public_endpoint(&self) -> Result<(String, u16), String> {
        let url = url::Url::parse(&self.public_url)
            .map_err(|error| format!("server.http.public_url is invalid: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("server.http.public_url must use http or https".to_string());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("server.http.public_url must not contain credentials".to_string());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("server.http.public_url must not contain query or fragment".to_string());
        }
        let host = url
            .host_str()
            .filter(|host| !host.trim().is_empty())
            .ok_or_else(|| "server.http.public_url must contain a host".to_string())?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "server.http.public_url must contain a valid port".to_string())?;
        Ok((host.to_string(), port))
    }
}

impl MediaServerConf {
    fn normalize(&mut self) {
        let advertised_host = match self {
            Self::Single {
                advertised_host, ..
            }
            | Self::Multi {
                advertised_host, ..
            } => advertised_host,
        };
        *advertised_host = advertised_host.trim().to_string();
    }

    pub fn listener_conf(&self) -> Result<MediaListenerConf, String> {
        match self {
            Self::Single {
                listen_ip,
                advertised_host,
                port,
            } => {
                validate_advertised_host(advertised_host)?;
                if *port == 0 {
                    return Err("server.media.port must be greater than zero".to_string());
                }
                Ok(MediaListenerConf {
                    mode: MediaListenerMode::Single,
                    bind_ip: *listen_ip,
                    advertised_host: advertised_host.clone(),
                    single_port: *port,
                    port_range: MediaPortRange {
                        start: *port,
                        end: *port,
                    },
                    reservation_timeout_secs: 0,
                })
            }
            Self::Multi {
                listen_ip,
                advertised_host,
                port_range,
                reservation_timeout_secs,
            } => {
                validate_advertised_host(advertised_host)?;
                if *reservation_timeout_secs == 0 {
                    return Err(
                        "server.media.reservation_timeout_secs must be greater than zero"
                            .to_string(),
                    );
                }
                if port_range.start < 1024 || port_range.start > port_range.end {
                    return Err(
                        "server.media.port_range must be ordered and start at or above 1024"
                            .to_string(),
                    );
                }
                let slot_count = usize::from(port_range.end - port_range.start) + 1;
                if slot_count > 1024 {
                    return Err(
                        "server.media.port_range must contain at most 1024 ports".to_string()
                    );
                }
                Ok(MediaListenerConf {
                    mode: MediaListenerMode::Multi,
                    bind_ip: *listen_ip,
                    advertised_host: advertised_host.clone(),
                    single_port: 0,
                    port_range: *port_range,
                    reservation_timeout_secs: *reservation_timeout_secs,
                })
            }
        }
    }
}

fn validate_advertised_host(host: &str) -> Result<(), String> {
    if host.trim().is_empty() {
        return Err("server.media.advertised_host must not be empty".to_string());
    }
    if host.contains("://") || host.contains('/') {
        return Err(
            "server.media.advertised_host must be a host without scheme or path".to_string(),
        );
    }
    Ok(())
}

impl ServerConf {
    pub fn init_by_conf() -> Self {
        let mut server_conf = ServerConf::conf();
        server_conf.http.public_url = server_conf
            .http
            .public_url
            .trim_end_matches('/')
            .to_string();
        server_conf.media.normalize();
        server_conf
    }

    pub fn media_listener_conf(&self) -> Result<MediaListenerConf, String> {
        self.media.listener_conf()
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("server.name must not be empty".to_string());
        }
        if self.http.listen_addr.port() == 0 {
            return Err("server.http.listen_addr port must not be zero".to_string());
        }
        let _ = self.http.public_endpoint()?;
        let public_is_https = self.http.public_url.starts_with("https://");
        if public_is_https != self.http.tls.enabled {
            return Err(
                "server.http.public_url scheme must match server.http.tls.enabled".to_string(),
            );
        }
        validate_tls_files(
            "server.http.tls",
            self.http.tls.enabled,
            &self.http.tls.certificate_path,
            &self.http.tls.private_key_path,
        )?;
        if self.grpc.listen_addr.port() == 0 {
            return Err("server.grpc.listen_addr port must not be zero".to_string());
        }
        if self.grpc.advertised_addr.port() == 0 {
            return Err("server.grpc.advertised_addr port must not be zero".to_string());
        }
        validate_tls_files(
            "server.grpc.tls",
            self.grpc.tls.enabled,
            &self.grpc.tls.certificate_path,
            &self.grpc.tls.private_key_path,
        )?;
        let media = self.media_listener_conf()?;
        validate_listener_port_conflicts(
            &media,
            self.http.listen_addr.port(),
            self.grpc.listen_addr.port(),
        )
    }
}

fn validate_tls_files(
    field: &str,
    enabled: bool,
    certificate_path: &PathBuf,
    private_key_path: &PathBuf,
) -> Result<(), String> {
    if enabled
        && (certificate_path.as_os_str().is_empty() || private_key_path.as_os_str().is_empty())
    {
        return Err(format!(
            "{field} certificate_path and private_key_path are required when enabled"
        ));
    }
    Ok(())
}

fn validate_listener_port_conflicts(
    media: &MediaListenerConf,
    http_port: u16,
    grpc_port: u16,
) -> Result<(), String> {
    if http_port == grpc_port {
        return Err("server.http and server.grpc listen ports conflict".to_string());
    }
    let media_conflicts = match media.mode {
        MediaListenerMode::Single => {
            media.single_port == http_port || media.single_port == grpc_port
        }
        MediaListenerMode::Multi => {
            (media.port_range.start..=media.port_range.end).contains(&http_port)
                || (media.port_range.start..=media.port_range.end).contains(&grpc_port)
        }
    };
    if media_conflicts {
        return Err("server.media listen ports conflict with HTTP or gRPC".to_string());
    }
    Ok(())
}

impl CheckFromConf for ServerConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        self.validate().map_err(FieldCheckError::BizError)
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

#[cfg(test)]
mod tests {
    use crate::general::cfg::{MediaListenerMode, MediaPortRange, ServerConf, StreamConf};
    use base::cfg_lib::conf::init_cfg;

    const SINGLE_SERVER_YAML: &str = r#"
name: stream-test
http:
  listen_addr: 0.0.0.0:28570
  public_url: http://127.0.0.1:28570
grpc:
  listen_addr: 127.0.0.1:19082
  advertised_addr: 127.0.0.1:19082
media:
  mode: single
  listen_ip: 0.0.0.0
  advertised_host: 127.0.0.1
  port: 28568
"#;

    const MULTI_SERVER_YAML: &str = r#"
name: stream-test
http:
  listen_addr: 0.0.0.0:28570
  public_url: http://127.0.0.1:28570
grpc:
  listen_addr: 127.0.0.1:19082
  advertised_addr: 127.0.0.1:19082
media:
  mode: multi
  listen_ip: 0.0.0.0
  advertised_host: 127.0.0.1
  port_range:
    start: 28600
    end: 28663
  reservation_timeout_secs: 30
"#;

    fn parse_server(yaml: &str) -> Result<ServerConf, base::serde_yaml::Error> {
        base::serde_yaml::from_str(yaml)
    }

    //   hls 与 flv: 都为false时，触发panic
    #[test]
    fn test_check_init_conf() {
        init_cfg("config.yml".to_string());
        let cf: ServerConf = ServerConf::init_by_conf();
        println!("{:?}", cf);
    }

    #[test]
    fn stream_timeout_defaults_and_boundaries_are_validated() {
        let defaults = StreamConf {
            in_wait_timeout: 4,
            out_idle_timeout: 20,
        };
        assert_eq!(defaults.resolve_media_timeouts(None, None), Ok((4, 20)));
        assert_eq!(
            defaults.resolve_media_timeouts(Some(1), Some(12)),
            Ok((1, 12))
        );
        assert_eq!(
            defaults.resolve_media_timeouts(Some(30), Some(120)),
            Ok((30, 120))
        );
        assert!(defaults.resolve_media_timeouts(Some(0), None).is_err());
        assert!(defaults.resolve_media_timeouts(Some(31), None).is_err());
        assert!(defaults.resolve_media_timeouts(None, Some(11)).is_err());
        assert!(defaults.resolve_media_timeouts(None, Some(121)).is_err());
    }

    #[test]
    fn single_and_multi_listener_configs_are_validated() {
        let server = parse_server(SINGLE_SERVER_YAML).unwrap();
        server.validate().unwrap();
        let single = server.media_listener_conf().unwrap();
        assert_eq!(single.mode, MediaListenerMode::Single);
        assert_eq!(single.single_port, 28_568);

        let server = parse_server(MULTI_SERVER_YAML).unwrap();
        server.validate().unwrap();
        assert_eq!(
            server.media_listener_conf().unwrap().port_range,
            MediaPortRange {
                start: 28_600,
                end: 28_663
            }
        );
    }

    #[test]
    fn legacy_and_mode_irrelevant_fields_are_rejected() {
        let legacy = format!("{SINGLE_SERVER_YAML}rtp_port: 28568\n");
        assert!(parse_server(&legacy).is_err());

        let single_with_range =
            format!("{SINGLE_SERVER_YAML}  port_range:\n    start: 28600\n    end: 28663\n");
        assert!(parse_server(&single_with_range).is_err());

        let multi_with_port = format!("{MULTI_SERVER_YAML}  port: 28568\n");
        assert!(parse_server(&multi_with_port).is_err());
    }

    #[test]
    fn url_tls_addresses_and_port_conflicts_are_rejected() {
        let mut tls_mismatch = parse_server(SINGLE_SERVER_YAML).unwrap();
        tls_mismatch.http.public_url = "https://127.0.0.1:28570".to_string();
        assert!(tls_mismatch.validate().is_err());

        let mut invalid_public_url = parse_server(SINGLE_SERVER_YAML).unwrap();
        invalid_public_url.http.public_url = "tcp://127.0.0.1:28570".to_string();
        assert!(invalid_public_url.validate().is_err());

        let mut missing_tls_files = parse_server(SINGLE_SERVER_YAML).unwrap();
        missing_tls_files.http.public_url = "https://127.0.0.1:28570".to_string();
        missing_tls_files.http.tls.enabled = true;
        assert!(missing_tls_files.validate().is_err());

        let mut zero_port = parse_server(SINGLE_SERVER_YAML).unwrap();
        zero_port.grpc.advertised_addr = "127.0.0.1:0".parse().unwrap();
        assert!(zero_port.validate().is_err());

        let mut conflict = parse_server(SINGLE_SERVER_YAML).unwrap();
        conflict.grpc.listen_addr = "127.0.0.1:28570".parse().unwrap();
        assert!(conflict.validate().is_err());

        let mut media_conflict = parse_server(MULTI_SERVER_YAML).unwrap();
        media_conflict.http.listen_addr = "0.0.0.0:28600".parse().unwrap();
        assert!(media_conflict.validate().is_err());
    }
}
