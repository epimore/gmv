use base::chrono::{DateTime, Local};

pub struct MediaParam {
    pub start_time:  DateTime<Local>,
    pub video: Option<Video>,
    pub audio: Option<Audio>,
}

pub struct Video {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub timescale: u32,
    pub bandwidth: u32,
}

pub struct Audio {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub timescale: u32,
    pub bandwidth: u32,
}
