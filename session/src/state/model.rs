use base::serde::{Deserialize, Serialize};

use crate::gb::handler::parser::xml::KV2Model;
use crate::state;
use anyhow::anyhow;
use base::exception::GlobalError::SysErr;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use utoipa::ToSchema;
use shared::info::codec::Codec;
use shared::info::filter::Filter;
use shared::info::media_info_ext::MediaType;
use shared::info::output::{OutputEnum, OutputKind};

#[derive(ToSchema,Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct StreamQo {
    /// 媒体流ID
    pub stream_id: String,
    /// 输出类型
    pub media_type: Option<OutputEnum>,
}


#[derive(ToSchema,Serialize, Deserialize, Debug,Copy, Clone)]
#[serde(crate = "base::serde")]
/// 传输方式 默认udp 模式, TcpPassive 被动模式,TcpActive 主动模式
pub enum TransMode {
    Udp,
    TcpActive,
    TcpPassive,
}
#[derive(ToSchema,Debug, Deserialize, Serialize,Clone)]
#[serde(crate = "base::serde")]
pub struct CustomMediaConfig {
    /// 媒体流输出信息
    pub output: OutputKind,
    /// 媒体流转码信息
    pub codec: Option<Codec>,
    /// 媒体流过滤信息
    pub filter: Filter,
}

#[derive(ToSchema,Debug, Deserialize, Serialize)]
#[serde(crate = "base::serde")]
pub struct PlayLiveModel {
    /// 设备id
    pub device_id: String,
    /// 通道id
    pub channel_id: Option<String>,
    /// 传输方式：udp, tcp_active, tcp_passive
    pub trans_mode: Option<TransMode>,
    /// 自定义媒体处理：如转码、过滤、输出格式等
    pub custom_media_config: Option<CustomMediaConfig>,
}

#[derive(ToSchema,Debug, Deserialize, Serialize,Clone)]
#[serde(crate = "base::serde")]
pub struct PlayBackModel {
    /// 设备ID
    pub device_id: String,
    /// 通道ID
    pub channel_id: Option<String>,
    /// 媒体流传输方式
    pub trans_mode: Option<TransMode>,
    /// 媒体流自定义处理信息
    pub custom_media_config: Option<CustomMediaConfig>,
    /// 历史视频回放开始时间
    pub st: u32,
    /// 历史视频回放结束时间
    pub et: u32,
}

#[derive(ToSchema,Debug, Deserialize, Serialize)]
#[serde(crate = "base::serde")]
#[allow(non_snake_case)]
pub struct PlaySeekModel {
    /// 媒体流ID
    pub streamId: String,
    /// 媒体流拖动：单位S
    pub seekSecond: u32,
}

#[derive(ToSchema,Debug, Deserialize, Serialize)]
#[serde(crate = "base::serde")]
#[allow(non_snake_case)]
pub struct PlaySpeedModel {
    /// 媒体流ID
    pub streamId: String,
    /// 媒体流倍速播放：0.5/1/2/4
    pub speedRate: f32,
}

#[derive(ToSchema,Debug, Deserialize, Serialize, Default)]
#[serde(crate = "base::serde")]
#[allow(non_snake_case)]
pub struct PtzControlModel {
    /// 设备ID
    pub deviceId: String,
    /// 通道ID
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

#[derive(ToSchema,Debug, Deserialize, Serialize)]
#[serde(crate = "base::serde")]
#[allow(non_snake_case)]
pub struct StreamInfo {
    pub streamId: String,
    pub flv: String,
    pub m3u8: String,
}

impl StreamInfo {
    pub fn build(stream_id: String, node_name: String) -> Self {
        let stream_conf = state::StreamConf::get_stream_conf();
        match stream_conf.get_proxy_addr() {
            None => {
                let node_stream = stream_conf.get_node_map().get(&node_name).unwrap();
                Self {
                    flv: format!("http://{}:{}/{node_name}/play/{stream_id}.flv", node_stream.pub_ip, node_stream.local_port),
                    m3u8: format!("http://{}:{}/{node_name}/play/{stream_id}.m3u8", node_stream.pub_ip, node_stream.local_port),
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

#[derive(ToSchema,Debug, Deserialize, Serialize, Default)]
#[serde(crate = "base::serde")]
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
