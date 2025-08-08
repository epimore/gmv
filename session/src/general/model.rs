use common::serde::{Deserialize, Serialize};

use crate::gb::handler::parser::xml::KV2Model;
use crate::general;
use anyhow::anyhow;
use common::exception::GlobalError::SysErr;
use common::exception::{GlobalResult, GlobalResultExt};
use common::log::error;
use shared::info::codec::Codec;
use shared::info::filter::Filter;
use shared::info::io::Output;

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct SingleParam<T> {
    pub param: T,
}

pub enum StreamMode {
    Udp,
    TcpActive,
    TcpPassive,
}

impl StreamMode {
    pub fn build(m: u8) -> GlobalResult<Self> {
        match m {
            0 => { Ok(StreamMode::Udp) }
            1 => { Ok(StreamMode::TcpActive) }
            2 => { Ok(StreamMode::TcpPassive) }
            _ => { Err(SysErr(anyhow!("无效流模式"))) }
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamNode {
    pub stream_id: String,
    pub stream_server: String,
}

// 传输方式 默认udp 模式, TcpPassive 被动模式,TcpActive 主动模式
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum TransMode {
    Udp,
    TcpActive,
    TcpPassive,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
pub struct CustomMediaConfig {
    pub output: Output,
    pub codec: Codec,
    pub filter: Filter,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
pub struct PlayLiveModel {
    pub device_id: String,
    pub channel_id: Option<String>,
    pub trans_mode: Option<TransMode>,
    pub custom_media_config: Option<CustomMediaConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
pub struct PlayBackModel {
    pub device_id: String,
    pub channel_id: Option<String>,
    pub trans_mode: Option<TransMode>,
    pub custom_media_config: Option<CustomMediaConfig>,
    pub st: u32,
    pub et: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PlaySeekModel {
    pub streamId: String,
    pub seekSecond: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PlaySpeedModel {
    pub streamId: String,
    pub speedRate: f32,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PtzControlModel {
    pub deviceId: String,
    pub channelId: String,
    ///镜头左移右移 0:停止 1:左移 2:右移
    pub leftRight: u8,
    ///镜头上移下移 0:停止 1:上移 2:下移
    pub upDown: u8,
    ///镜头放大缩小 0:停止 1:缩小 2:放大
    pub inOut: u8,
    ///水平移动速度：1-255
    pub horizonSpeed: u8,
    ///垂直移动速度：0-255
    pub verticalSpeed: u8,
    ///焦距缩放速度：0-15
    pub zoomSpeed: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct StreamInfo {
    pub streamId: String,
    pub flv: String,
    pub m3u8: String,
}

impl StreamInfo {
    pub fn build(stream_id: String, node_name: String) -> Self {
        let stream_conf = general::StreamConf::get_stream_conf();
        match stream_conf.get_proxy_addr() {
            None => {
                let node_stream = stream_conf.get_node_map().get(&node_name).unwrap();
                Self {
                    flv: format!("http://{}:{}/{node_name}/play/{stream_id}.flv", node_stream.get_pub_ip(), node_stream.get_local_port()),
                    m3u8: format!("http://{}:{}/{node_name}/play/{stream_id}.m3u8", node_stream.get_pub_ip(), node_stream.get_local_port()),
                    streamId: stream_id,
                }
            }
            Some(addr) => {
                Self {
                    flv: format!("{addr}/{node_name}/play/{stream_id}.flv"),
                    m3u8: format!("{addr}/{node_name}/play/{stream_id}.m3u8"),
                    streamId: stream_id,
                }
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct AlarmInfo {
    pub priority: u8,
    pub method: u8,
    pub alarmType: u8,
    pub timeStr: String,
    pub deviceId: String,
    pub channelId: String,
}

impl KV2Model for AlarmInfo {
    fn kv_to_model(arr: Vec<(String, String)>) -> GlobalResult<Self> {
        use crate::gb::handler::parser::xml::*;
        let mut model = AlarmInfo::default();
        for (k, v) in arr {
            match &k[..] {
                NOTIFY_DEVICE_ID => {
                    model.channelId = v;
                }
                NOTIFY_ALARM_PRIORITY => {
                    model.priority = v.parse::<u8>().hand_log(|msg| error!("{msg}"))?;
                }
                NOTIFY_ALARM_TIME => {
                    model.timeStr = v;
                }
                NOTIFY_ALARM_METHOD => {
                    model.method = v.parse::<u8>().hand_log(|msg| error!("{msg}"))?;
                }
                NOTIFY_INFO_ALARM_TYPE => {
                    model.alarmType = v.parse::<u8>().hand_log(|msg| error!("{msg}"))?;
                }
                &_ => {}
            }
        }
        Ok(model)
    }
}
