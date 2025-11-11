use crate::info::format::{CMaf, Flv, HlsTs, Mp4, RtpEnc, RtpFrame, RtpPs, Ts};
use base::serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(ToSchema,Serialize, Deserialize, Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
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

#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
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

#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct LocalMp4Output {
    pub fmt: Mp4,
    pub path: String,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct LocalTsOutput {
    pub fmt: Ts,
    pub path: String,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HttpFlvOutput {
    pub fmt: Flv,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RtmpOutput {
    pub fmt: Flv,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HlsTsOutput {
    pub fmt: HlsTs,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct HlsFmp4Output {
    pub fmt: CMaf,
}

#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct DashFmp4Output {
    pub fmt: CMaf,
}

#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct RtspOutput {
    pub fmt: RtpFrame,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct Gb28181FrameOutput {
    pub fmt: RtpFrame,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct Gb28181PsOutput {
    pub fmt: RtpPs,
}
#[derive(ToSchema,Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct WebRtcOutput {
    pub fmt: RtpEnc,
}
