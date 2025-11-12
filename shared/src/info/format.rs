use base::serde::{Deserialize, Serialize};

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Mp4 {}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct CMaf {
    //    // 设置关键的 muxer options
    //     av_dict_set(&opts, "movflags", "frag_keyframe+frag_custom+dash+empty_moov", 0); // 组合flags
    //     av_dict_set(&opts, "min_frag_duration", "2000000", 0); // 目标分片时长2秒
    pub min_frag_duration: u64,
    pub movflags: String,
}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Flv {}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpFrame {}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpPs {}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpEnc {}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Ts {}
#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct HlsTs {}
