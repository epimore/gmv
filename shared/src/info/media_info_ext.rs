use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::serde::{Deserialize, Serialize};

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
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "base::serde")]
pub struct MediaExt {
    pub mt: MediaType,
    pub tp_code: u8,
    pub tp_val: String,
    pub link_ssrc: Option<u32>,
}