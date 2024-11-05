use std::collections::{HashMap};
use std::net::Ipv4Addr;
use common::cfg_lib;
use common::cfg_lib::conf;
use common::serde_yaml;
use common::constructor::Get;
use common::once_cell::sync::OnceCell;
use common::serde::{Deserialize};

pub mod model;
pub mod cache;
pub mod http;

#[derive(Debug, Get, Deserialize)]
#[conf(prefix = "server.session")]
pub struct SessionConf {
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}

impl SessionConf {
    pub fn get_session_conf() -> Self {
        SessionConf::conf()
    }
}

#[derive(Debug, Get, Deserialize)]
#[conf(prefix = "server.stream")]
pub struct StreamConf {
    proxy_enable: bool,
    proxy_addr: Option<String>,
    //node_name:StreamNode
    node_map: HashMap<String, StreamNode>,
    nodes: Vec<StreamNode>,
}

#[derive(Debug, Get, Deserialize, Clone)]
pub struct StreamNode {
    name: String,
    local_ip: Ipv4Addr,
    local_port: u16,
    pub_ip: Ipv4Addr,
    pub_port: u16,
}
static CELL: OnceCell<StreamConf> = OnceCell::new();
impl StreamConf {

    pub fn get_stream_conf() -> &'static Self {
        CELL.get_or_init(||{
            let mut conf: Self = StreamConf::conf();
            let mut node_map = HashMap::new();
            for node in &conf.nodes {
                if let Some(old) = node_map.insert(node.name.clone(), node.clone()) {
                    panic!("配置server.stream.nodes.name重复:{}:，建议使用s1,s2,s3等连续编号", old.name);
                }
            }
            if node_map.is_empty() {
                panic!("未配置流媒体信息")
            }
            conf.node_map = node_map;
            conf
        })
    }
}


#[cfg(test)]
mod tests {
    use crate::general::SessionConf;

    #[test]
    fn test_map_conf() {
        println!("{:?}", SessionConf::get_session_conf());
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