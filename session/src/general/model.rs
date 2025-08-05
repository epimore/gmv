use poem_openapi::{self, Object};
use poem_openapi::types::{ParseFromJSON, ToJSON, Type};
use common::serde::{Deserialize, Serialize};

use anyhow::anyhow;
use common::exception::GlobalError::SysErr;
use common::exception::{GlobalResult, GlobalResultExt};
use common::constructor::Get;
use common::log::error;
use crate::gb::handler::parser::xml::KV2Model;

use crate::general;

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

#[derive(Debug, Clone, Eq, PartialEq, Object)]
pub struct ResultMessageData<T: Type + ParseFromJSON + ToJSON> {
    code: u16,
    msg: Option<String>,
    data: Option<T>,
}

impl<T: Type + ParseFromJSON + ToJSON> ResultMessageData<T> {
    #[allow(dead_code)]
    pub fn build(code: u16, msg: String, data: T) -> Self {
        Self { code, msg: Some(msg), data: Some(data) }
    }

    pub fn build_success(data: T) -> Self {
        Self { code: 200, msg: Some("success".to_string()), data: Some(data) }
    }
    pub fn build_success_none() -> Self {
        Self { code: 200, msg: Some("success".to_string()), data: None }
    }
    pub fn build_failure() -> Self {
        Self { code: 500, msg: Some("failure".to_string()), data: None }
    }
    pub fn build_failure_msg(msg: String) -> Self {
        Self { code: 500, msg: Some(msg), data: None }
    }
}


#[derive(Object, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct StreamNode {
    pub stream_id: String,
    pub stream_server: String,
}

#[derive(Debug, Deserialize, Object, Serialize, Get)]
#[serde(crate = "common::serde")]
pub struct PlayLiveModel {
    #[oai(validator(min_length = "20", max_length = "20"))]
    device_id: String,
    #[oai(validator(min_length = "20", max_length = "20"))]
    channel_id: Option<String>,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    trans_mode: Option<u8>,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    /// 媒体类型，默认flv,hls开启,(todo 2-mp4 3-webrtc ...)
    media_type: Option<u8>,
}

#[derive(Debug, Deserialize, Object, Serialize, Get)]
#[serde(crate = "common::serde")]
pub struct PlayBackModel {
    #[oai(validator(min_length = "20", max_length = "20"))]
    device_id: String,
    #[oai(validator(min_length = "20", max_length = "20"))]
    channel_id: Option<String>,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    trans_mode: Option<u8>,
    st: u32,
    et: u32,
}

#[derive(Debug, Deserialize, Object, Serialize, Get)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PlaySeekModel {
    #[oai(validator(min_length = "24", max_length = "32")
    )]
    streamId: String,
    #[oai(validator(maximum(value = "86400"), minimum(value = "1")))]
    seekSecond: u32,
}

#[derive(Debug, Deserialize, Object, Serialize, Get)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PlaySpeedModel {
    #[oai(validator(min_length = "24", max_length = "32")
    )]
    streamId: String,
    #[oai(validator(maximum(value = "8"), minimum(value = "0.25")))]
    speedRate: f32,
}

#[derive(Debug, Deserialize, Object, Serialize, Default)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct PtzControlModel {
    #[oai(validator(min_length = "20", max_length = "20"))]
    pub deviceId: String,
    #[oai(validator(min_length = "20", max_length = "20"))]
    pub channelId: String,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    ///镜头左移右移 0:停止 1:左移 2:右移
    pub leftRight: u8,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    ///镜头上移下移 0:停止 1:上移 2:下移
    pub upDown: u8,
    #[oai(validator(maximum(value = "2"), minimum(value = "0")))]
    ///镜头放大缩小 0:停止 1:缩小 2:放大
    pub inOut: u8,
    #[oai(validator(maximum(value = "255"), minimum(value = "0")))]
    ///水平移动速度：1-255
    pub horizonSpeed: u8,
    #[oai(validator(maximum(value = "255"), minimum(value = "0")))]
    ///垂直移动速度：0-255
    pub verticalSpeed: u8,
    #[oai(validator(maximum(value = "15"), minimum(value = "0")))]
    ///焦距缩放速度：0-15
    pub zoomSpeed: u8,
}

#[derive(Debug, Deserialize, Object, Serialize)]
#[serde(crate = "common::serde")]
#[allow(non_snake_case)]
pub struct StreamInfo {
    streamId: String,
    flv: String,
    m3u8: String,
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

#[derive(Debug, Deserialize, Object, Serialize, Default)]
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

#[cfg(test)]
mod test {
    use poem_openapi::payload::Json;
    use poem_openapi::types::ToJSON;

    use crate::general::model::{ResultMessageData, StreamInfo};

    #[test]
    fn t1() {
        let m = StreamInfo {
            streamId: "streamId".to_string(),
            flv: "streamId".to_string(),
            m3u8: "streamId".to_string(),
        };
        let data = ResultMessageData::build_success(m);
        println!("{:#?}", Json(data).to_json_string());
    }
}