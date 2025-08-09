use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub enum Codec {
    //video
    Mpeg4,
    H264,
    SvacVideo,
    H265,
    //audio
    G711a,
    G711u,
    G7221,
    G7231,
    G729,
    SvacAudio,
    Aac,
}