use crate::info::format::{CMaf, Flv, HlsTs, Mp4, RtpEnc, RtpFrame, RtpPs, Ts};
use crate::info::muxer::MuxerEnum;
use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
#[serde(crate = "base::serde")]
pub enum OutputEnum {
    HttpFlv,
    Rtmp,
    DashFmp4,
    HlsFmp4,
    HlsTs,
    Rtsp,
    Gb28181Frame,
    Gb28181Ps,
    WebRtc,
    LocalMp4,
    LocalTs,
}
impl OutputEnum {
    pub fn to_muxer_enum(&self) -> MuxerEnum {
        match self {
            OutputEnum::HttpFlv => MuxerEnum::Flv,
            OutputEnum::Rtmp => MuxerEnum::Flv,
            OutputEnum::DashFmp4 => MuxerEnum::FMp4,
            OutputEnum::HlsFmp4 => MuxerEnum::FMp4,
            OutputEnum::HlsTs => MuxerEnum::Ts,
            OutputEnum::Rtsp => MuxerEnum::RtpFrame,
            OutputEnum::Gb28181Frame => MuxerEnum::RtpFrame,
            OutputEnum::Gb28181Ps => MuxerEnum::RtpPs,
            OutputEnum::WebRtc => MuxerEnum::RtpEnc,
            OutputEnum::LocalMp4 => MuxerEnum::Mp4,
            OutputEnum::LocalTs => MuxerEnum::Ts,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub enum OutputKind {
    HttpFlv(HttpFlvOutput),
    Rtmp(RtmpOutput),
    DashFmp4(DashFmp4Output),
    HlsFmp4(HlsFmp4Output),
    HlsTs(HlsTsOutput),
    Rtsp(RtspOutput),
    Gb28181Frame(Gb28181FrameOutput),
    Gb28181Ps(Gb28181PsOutput),
    WebRtc(WebRtcOutput),
    LocalMp4(LocalMp4Output),
    LocalTs(LocalTsOutput),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct LocalMp4Output {
    pub fmt: Mp4,
    pub path: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct LocalTsOutput {
    pub fmt: Ts,
    pub path: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HttpFlvOutput {
    pub fmt: Flv,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RtmpOutput {
    pub fmt: Flv,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HlsTsOutput {
    pub fmt: HlsTs,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HlsFmp4Output {
    pub fmt: CMaf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct DashFmp4Output {
    pub fmt: CMaf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RtspOutput {
    pub fmt: RtpFrame,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct Gb28181FrameOutput {
    pub fmt: RtpFrame,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct Gb28181PsOutput {
    pub fmt: RtpPs,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct WebRtcOutput {
    pub fmt: RtpEnc,
}
