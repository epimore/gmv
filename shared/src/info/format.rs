use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct Muxer {
    pub flv: Option<Flv>,
    pub mp4: Option<Mp4>,
    pub ts: Option<Ts>,
    pub rtp_frame: Option<RtpFrame>,
    pub rtp_ps: Option<RtpPs>,
    pub rtp_enc: Option<RtpEnc>,
    pub frame: Option<Frame>,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Frame {}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(crate = "base::serde")]
pub struct Mp4 {}
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub enum GB28181MuxerType {
    RtpFrame,
    RtpPs,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub enum WebRtcMuxerType {
    RtpFrame,
    RtpEnc,
}

#[derive(Serialize, Deserialize, Debug, Clone, Ord, PartialOrd, Eq, PartialEq,Copy )]
#[serde(crate = "base::serde")]
pub enum MuxerType {
    None,
    Flv,
    Mp4,
    Ts,
    RtpFrame,
    RtpPs,
    RtpEnc,
    Frame,
}
impl From<GB28181MuxerType> for MuxerType {
    fn from(muxer: GB28181MuxerType) -> Self {
        match muxer {
            GB28181MuxerType::RtpFrame => MuxerType::RtpFrame,
            GB28181MuxerType::RtpPs => MuxerType::RtpPs,
        }
    }
}

impl From<WebRtcMuxerType> for MuxerType {
    fn from(muxer: WebRtcMuxerType) -> Self {
        match muxer {
            WebRtcMuxerType::RtpFrame => MuxerType::RtpFrame,
            WebRtcMuxerType::RtpEnc => MuxerType::RtpEnc,
        }
    }
}
