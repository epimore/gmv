use base::serde::{Deserialize, Serialize};

// #[derive(Serialize, Deserialize, Debug, Default, Clone)]
// #[serde(crate = "base::serde")]
// pub struct Muxer {
//     pub flv: Option<Flv>,
//     pub mp4: Option<Mp4>,
//     pub mp4_dash: Option<CMaf>,
//     pub ts: Option<Ts>,
//     pub hls: Option<HlsTs>,
//     pub rtp_frame: Option<RtpFrame>,
//     pub rtp_ps: Option<RtpPs>,
//     pub rtp_enc: Option<RtpEnc>,
//     pub frame: Option<Frame>,
// }
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Mp4 {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct CMaf {
    //    // 设置关键的 muxer options
    //     av_dict_set(&opts, "movflags", "frag_keyframe+frag_custom+dash+empty_moov", 0); // 组合flags
    //     av_dict_set(&opts, "min_frag_duration", "2000000", 0); // 目标分片时长2秒
    pub min_frag_duration: u64,
    pub movflags: String,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Flv {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpFrame {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpPs {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct RtpEnc {}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Ts {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct HlsTs {}
// 
// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[serde(crate = "base::serde")]
// pub enum GB28181MuxerType {
//     RtpFrame,
//     RtpPs,
// }
// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[serde(crate = "base::serde")]
// pub enum WebRtcMuxerType {
//     RtpFrame,
//     RtpEnc,
// }
// 
// #[derive(Serialize, Deserialize, Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Copy)]
// #[serde(crate = "base::serde")]
// pub enum MuxerType {
//     None,
//     Flv,
//     Mp4,
//     Mp4Dash,
//     Ts,
//     Hls,
//     RtpFrame,
//     RtpPs,
//     RtpEnc,
//     Frame,
// }
// impl From<GB28181MuxerType> for MuxerType {
//     fn from(muxer: GB28181MuxerType) -> Self {
//         match muxer {
//             GB28181MuxerType::RtpFrame => MuxerType::RtpFrame,
//             GB28181MuxerType::RtpPs => MuxerType::RtpPs,
//         }
//     }
// }
// 
// impl From<WebRtcMuxerType> for MuxerType {
//     fn from(muxer: WebRtcMuxerType) -> Self {
//         match muxer {
//             WebRtcMuxerType::RtpFrame => MuxerType::RtpFrame,
//             WebRtcMuxerType::RtpEnc => MuxerType::RtpEnc,
//         }
//     }
// }
