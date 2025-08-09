use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(crate = "base::serde")]
pub enum MediaType {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(crate = "base::serde")]
pub struct MediaMap {
    pub ssrc: u32,
    pub ext: MediaExt,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(crate = "base::serde")]
pub struct MediaExt {
    pub mt: MediaType,
    pub tp_code: u8,
    pub tp_val: String,
    pub link_ssrc: Option<u32>,
}