use std::collections::{HashMap};
use std::fs;
use std::net::Ipv4Addr;
use std::sync::OnceLock;
use base::{serde_default};
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::once_cell::sync::OnceCell;
use base::serde::{Deserialize};
use url::Url;

pub mod model;
pub mod session;
pub mod schedule;
pub mod runner;

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.alarm", check)]
pub struct AlarmConf {
    pub enable: bool,
    pub push_url: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: u8,
}
serde_default!(default_priority, u8, 4);
static ALARM_CONF: OnceLock<AlarmConf> = OnceLock::new();

impl AlarmConf {
    pub fn get_alarm_conf() -> &'static Self {
        ALARM_CONF.get_or_init(|| {
            AlarmConf::conf()
        })
    }
}

impl CheckFromConf for AlarmConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.enable {
            if self.push_url.is_none() || self.push_url.as_ref().unwrap().is_empty() {
                return Err(FieldCheckError::BizError("server.alarm.push_url不能为空".to_string()));
            }

            if Url::parse(self.push_url.as_ref().unwrap()).is_err() {
                return Err(FieldCheckError::BizError("server.alarm.push_url非有效的url地址".to_string()));
            }
        }

        if self.priority == 0 || self.priority > 4 {
            return Err(FieldCheckError::BizError("server.alarm.priority必须为1|2|3|4".to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.stream")]
pub struct StreamConf {
    #[serde(default = "default_node_map")]
    pub node_map: HashMap<String, StreamNode>,
    pub nodes: Vec<StreamNode>,
}
serde_default!(default_node_map, HashMap<String, StreamNode>, HashMap::new());
#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
pub struct StreamNode {
    pub name: String,
    pub local_ip: Ipv4Addr,
    pub local_port: u16,
    pub pub_ip: Ipv4Addr,
    pub pub_port: u16,
}

static CELL: OnceCell<StreamConf> = OnceCell::new();

impl StreamConf {
    pub fn get_stream_conf() -> &'static Self {
        CELL.get_or_init(|| {
            let mut conf: Self = StreamConf::conf();
            for node in &conf.nodes {
                if let Some(old) = conf.node_map.insert(node.name.clone(), node.clone()) {
                    panic!("配置server.stream.nodes.name重复:{}:，建议使用s1,s2,s3等连续编号", old.name);
                }
            }
            if conf.node_map.is_empty() {
                panic!("未配置流媒体信息")
            }
            conf
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.videos", check)]
pub struct DownloadConf {
    pub storage_path: String,
}
impl CheckFromConf for DownloadConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let dc = DownloadConf::conf();
        fs::create_dir_all(&dc.storage_path).map_err(|e| FieldCheckError::BizError(format!("create download dir failed: {}", e.to_string())))?;
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

    // #[test]
    // fn test_map_conf() {
    //     println!("{:?}", super::StreamConf::get_stream_conf());
    // }

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