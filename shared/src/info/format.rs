use common::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "common::serde")]
pub struct Muxer {
    pub flv: Option<Flv>,
    pub mp4: Option<Mp4>,
    pub ts: Option<Ts>,
    pub rtp_frame: Option<RtpFrame>,
    pub rtp_ps: Option<RtpPs>,
    pub rtp_enc: Option<RtpEnc>,
    pub frame: Option<Frame>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Frame {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Mp4 {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Flv {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpFrame {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpPs {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpEnc {}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Ts {}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub enum MuxerTypeExt {
    Flv(Flv),
    Mp4(Mp4),
    Ts(Ts),
    RtpFrame(RtpFrame),
    RtpPs(RtpPs),
    RtpEnc(RtpEnc),
    Frame(Frame),
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, Ord, PartialEq, PartialOrd,Copy)]
#[serde(crate = "common::serde")]
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