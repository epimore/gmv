pub mod model;
mod cache;
pub mod http;

use std::net::Ipv4Addr;
use common::yaml_rust::Yaml;
use constructor::Get;

#[derive(Debug, Get)]
pub struct SessionConf {
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}

impl SessionConf {
    pub fn get_session_conf(cfg: &Yaml) -> Self {
        if cfg.is_badvalue() || cfg["server"].is_badvalue() || cfg["server"]["session"].is_badvalue() {
            panic!("server session config is invalid");
        }
        SessionConf {
            lan_ip: cfg["server"]["session"]["lan_ip"].as_str().expect("server session lan_ip config is invalid").parse::<Ipv4Addr>().expect("server session lan_ip IPV4 is invalid"),
            wan_ip: cfg["server"]["session"]["wan_ip"].as_str().expect("server session wan_ip config is invalid").parse::<Ipv4Addr>().expect("server session wan_ip IPV4 is invalid"),
            lan_port: cfg["server"]["session"]["lan_port"].as_i64().expect("server session lan_port config is invalid") as u16,
            wan_port: cfg["server"]["session"]["wan_port"].as_i64().expect("server session wan_port config is invalid") as u16,
        }
    }

    pub fn get_session_conf_by_cache() -> Self {
        let cfg = common::get_config().clone().get(0).expect("config file is invalid").clone();
        Self::get_session_conf(&cfg)
    }
}

pub struct StreamConf {}

#[cfg(test)]
mod tests {
    use crate::general::SessionConf;

    #[test]
    fn test_map_conf() {
        let cfg = common::get_config().clone().get(0).expect("config file is invalid").clone();
        println!("{:?}", SessionConf::get_session_conf(&cfg));
    }
}