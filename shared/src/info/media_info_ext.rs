use std::collections::HashMap;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::serde::{Deserialize, Serialize};
use crate::impl_check_empty;
use crate::info::format::MuxerType;
use std::sync::LazyLock;

static BITRATE_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    map.insert("1", "5.3");
    map.insert("2", "6.3");
    map.insert("3", "8");
    map.insert("4", "16");
    map.insert("5", "24");
    map.insert("6", "32");
    map.insert("7", "48");
    map.insert("8", "64");
    map.insert("9", "12");
    map.insert("10", "80");
    map.insert("11", "96");
    map.insert("12", "112");
    map.insert("13", "128");
    map.insert("14", "160");
    map.insert("15", "192");
    map.insert("16", "224");
    map.insert("17", "256");
    map.insert("18", "288");
    map.insert("19", "320");
    map.insert("20", "10.8");
    map.insert("21", "12.4");
    map.insert("22", "14");
    map.insert("23", "15.6");
    map.insert("24", "17.2");
    map.insert("25", "19.6");
    map.insert("26", "21.2");
    map.insert("27", "24.4");
    map.insert("28", "23.05");
    map.insert("29", "34");
    map.insert("30", "48.61");
    map
});
static SAMPLE_RATE_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    map.insert("1", "8");
    map.insert("2", "14");
    map.insert("3", "16");
    map.insert("4", "32");
    map.insert("5", "7");
    map.insert("6", "11");
    map.insert("7", "12");
    map.insert("8", "22");
    map.insert("9", "24");
    map.insert("10", "44");
    map.insert("11", "48");
    map.insert("12", "64");
    map.insert("13", "88");
    map.insert("14", "96");
    map.insert("15", "12.8");
    map.insert("16", "25.6");
    map.insert("17", "38.4");
    map
});

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "base::serde")]
pub enum MediaType {
    Video,
    Audio,
    //sdp other -> "text", "application" or "message"
}
impl MediaType {
    pub fn to_string(&self) -> String {
        match self {
            MediaType::Video => "video".to_string(),
            MediaType::Audio => "audio".to_string(),
        }
    }
    pub fn from_str(s: &str) -> GlobalResult<Self> {
        match s {
            "video" => Ok(MediaType::Video),
            "audio" => Ok(MediaType::Audio),
            _ => {
                Err(GlobalError::new_sys_error("unsupported media type", |msg| warn!("{msg}:{}",s)))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "base::serde")]
pub struct MediaMap {
    pub ssrc: u32,
    pub ext: MediaExt,
}
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct RtpEncrypt {}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct MediaExt {
    pub rtp_encrypt: Option<RtpEncrypt>,
    pub type_code: u8, //rtp payload type
    pub type_name: String, //rtp payload name
    pub clock_rate: i32, //时钟频率
    pub stream_number: Option<u8>, //gb28181自定义属性，流编号:0-主码流（高清流）1-子码率（标清流）
    pub video_params: VideoParams,
    pub audio_params: AudioParams,
}impl MediaExt{
    pub fn codec_from_psm(&self,codec_id:i32){
        if self.type_code == 96 {
            let codec = match codec_id {
                0x10 => { "mpeg4" }
                0x1B => {"h264"}
                0x80 => {"v.svac"}
                0x24 => {"h265"}
                0x90 => {"g711a"}
                0x91 => {"g711u"}
                0x92 => {"g7221"}
                0x93 => {"g7231"}
                0x99 => {"g729"}
                0x9B => {"a.svac"}
                0x0F => {"aac"}
                _ => {""}
            };
            info!("codec: {}", codec);
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct VideoParams {
    pub codec_id: Option<String>,   // AVCodecID (ffi::AVCodecID)
    pub resolution: Option<(i32, i32)>, //(W,H)
    pub fps: Option<i32>, //帧率
    pub bitrate_type: Option<String>, //1-固定码率(CBR);2-可变码率(VBR)
    pub bitrate: Option<i32>, //码率 kbps
}
impl_check_empty!(VideoParams,[codec_id,resolution,fps,bitrate_type,bitrate]);
impl VideoParams {
    pub fn map_video_codec(&mut self, item: &str) {
        match item {
            "1" => self.codec_id = Some("mpeg4".to_string()),
            "2" => self.codec_id = Some("h264".to_string()),
            "3" => self.codec_id = Some("svac".to_string()),
            "4" => self.codec_id = Some("3gp".to_string()),
            "5" => self.codec_id = Some("h265".to_string()),
            _ => warn!("Unknown video codec: {}", item),
        }
    }

    pub fn map_resolution(&mut self, item: &str) {
        match item {
            "1" => self.resolution = Some((176, 144)),
            "2" => self.resolution = Some((352, 288)),
            "3" => self.resolution = Some((704, 576)),
            "4" => self.resolution = Some((704, 576)),
            "5" => self.resolution = Some((1280, 720)),
            "6" => self.resolution = Some((1920, 1080)),
            _ => {
                if let Some((w, h)) = item.split_once('x') {
                    if let (Some(width), Some(height)) = (w.parse::<i32>().ok(), h.parse::<i32>().ok()) {
                        self.resolution = Some((width, height));
                        return;
                    }
                }
                warn!("Unknown resolution: {}", item);
            }
        }
    }

    pub fn map_fps(&mut self, item: &str) {
        if let Some(fps) = item.parse::<i32>().ok() {
            self.fps = Some(fps);
        } else {
            warn!("Unknown fps: {}", item);
        }
    }
    pub fn map_bitrate_type(&mut self, item: &str) {
        match item {
            "1" => self.bitrate_type = Some("CBR".to_string()),
            "2" => self.bitrate_type = Some("VBR".to_string()),
            _ => { warn!("Unknown bitrate type: {}", item); }
        }
    }

    pub fn map_bitrate(&mut self, item: &str) {
        if let Some(br) = item.parse::<i32>().ok() {
            self.bitrate = Some(br);
        } else {
            warn!("Unknown video bitrate: {}", item);
        }
    }
}
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "base::serde")]
pub struct AudioParams {
    pub codec_id: Option<String>,   // AVCodecID (ffi::AVCodecID)
    pub bitrate: Option<String>, //码率 kbps
    pub sample_rate: Option<String>, //采样率 kHz
    pub channel_count: i32,
    pub clock_rate: i32,
}
impl_check_empty!(AudioParams,[codec_id,bitrate,sample_rate]);
impl Default for AudioParams {
    fn default() -> Self {
        Self {
            codec_id: None,
            bitrate: None,
            sample_rate: None,
            channel_count: 1,
            clock_rate: 8000,
        }
    }
}
impl AudioParams {
    pub fn map_audio_codec(&mut self, item: &str) {
        match item {
            "1" => self.codec_id = Some("g711".to_string()),
            "2" => self.codec_id = Some("g723".to_string()),
            "3" => self.codec_id = Some("g729".to_string()),
            "4" => self.codec_id = Some("g722".to_string()),
            "5" => self.codec_id = Some("svac".to_string()),
            "6" => self.codec_id = Some("aac".to_string()),
            _ => warn!("Unknown audio codec: {}", item),
        }
    }

    pub fn map_bitrate(&mut self, item: &str) {
        if let Some(rate) = BITRATE_MAP.get(item) {
            self.bitrate = Some(rate.to_string());
        } else {
            warn!("Unknown audio bitrate: {}", item);
        }
    }

    pub fn map_sample_rate(&mut self, item: &str) {
        if let Some(rate) = SAMPLE_RATE_MAP.get(item) {
            self.sample_rate = Some(rate.to_string());
        } else {
            warn!("Unknown sample rate: {}", item);
        }
    }
}