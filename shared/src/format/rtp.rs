use common::serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpFrame {
}
#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpPs {
}
#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpEnc {
}

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum RtpPayloadType {
    Frame,
    Ps,
    RtpEnc,
}