use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
#[serde(crate = "base::serde")]
pub enum MuxerEnum {
    Flv,
    Mp4,
    Ts,
    CMaf,
    HlsTs,
    RtpFrame,
    RtpPs,
    RtpEnc,
}
