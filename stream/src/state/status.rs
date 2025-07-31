pub struct Active {
    pub play: bool,
    pub download: bool,
    pub transcode: FlvInfo,
}
pub struct FlvInfo {}
pub struct TsInfo {}
pub struct Mp4Info {}

pub enum MediaBox {
    Flv,
    Ts,
    Mp4,
    // RTP,
    // NAL,
    // PS,
    // ...
}