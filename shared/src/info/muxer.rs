use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
#[serde(crate = "base::serde")]
pub enum MuxerEnum {
    Flv,
    Mp4,
    Ts,
    FMp4,
    HlsTs,
    RtpFrame,
    RtpPs,
    RtpEnc,
}
