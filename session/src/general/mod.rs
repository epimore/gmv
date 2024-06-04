use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::ops::Index;
use common::yaml_rust::Yaml;
use constructor::Get;

pub mod model;
pub mod cache;
pub mod http;

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

#[derive(Get)]
pub struct StreamConf {
    proxy_addr: Option<String>,
    //node_name:StreamNode
    node_map: HashMap<String, StreamNode>,
}

#[derive(Get)]
pub struct StreamNode {
    local_ip: Ipv4Addr,
    local_port: u16,
    pub_ip: Ipv4Addr,
    pub_port: u16,
}

impl StreamConf {
    pub fn get_session_conf(cfg: &Yaml) -> Self {
        if cfg.is_badvalue() || cfg["server"].is_badvalue() || cfg["server"]["stream"].is_badvalue() {
            panic!("server stream config is invalid");
        }
        let en_proxy = cfg["server"]["stream"]["proxy_enable"].as_bool().expect("server stream config:proxy_enable is invalid");
        let mut proxy_addr = None;
        if en_proxy {
            let addr = cfg["server"]["stream"]["proxy_addr"].as_str().expect("server stream config:proxy_addr is invalid");
            proxy_addr = Some(addr.to_string())
        }
        let media = &cfg["server"]["stream"]["node"];
        let arr = media.as_vec().expect("server stream node config is invalid");
        let mut node_map = HashMap::new();
        for (index, val) in arr.iter().enumerate() {
            let name = val.index("name").as_str().expect(&format!("node-{index}: 获取name失败")).to_string();
            if node_map.contains_key(&name) {
                panic!("node-{index}:name重复，建议使用s1,s2,s3等连续编号");
            }
            let pub_ip = val.index("pub_ip").as_str().expect(&format!("node-{index}:公网ip错误")).parse::<Ipv4Addr>().expect("server session pub_ip IPV4 is invalid");
            let pub_port = val.index("pub_port").as_i64().expect(&format!("node-{index}:公网端口错误")) as u16;
            let local_ip = val.index("pub_ip").as_str().expect(&format!("node-{index}:局域网ip错误")).parse::<Ipv4Addr>().expect("server session local_ip IPV4 is invalid");
            let local_port = val.index("pub_port").as_i64().expect(&format!("node-{index}:局域网端口错误")) as u16;
            let node = StreamNode {
                local_ip,
                local_port,
                pub_ip,
                pub_port,
            };
            node_map.insert(name, node);
        }
        Self { proxy_addr, node_map }
    }

    pub fn get_session_conf_by_cache() -> Self {
        let cfg = common::get_config().clone().get(0).expect("config file is invalid").clone();
        Self::get_session_conf(&cfg)
    }
}


#[cfg(test)]
mod tests {
    use crate::general::SessionConf;

    #[test]
    fn test_map_conf() {
        let cfg = common::get_config().clone().get(0).expect("config file is invalid").clone();
        println!("{:?}", SessionConf::get_session_conf(&cfg));
    }

    fn print_banner(c: char) {
        let binary = match c {
            'G' => [
                0b11111,
                0b10000,
                0b10011,
                0b10001,
                0b11111,
            ],
            'M' => [
                0b10001,
                0b11011,
                0b10101,
                0b10001,
                0b10001,
            ],
            'V' => [
                0b10001,
                0b10001,
                0b01010,
                0b00100,
                0b00100,
            ],
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