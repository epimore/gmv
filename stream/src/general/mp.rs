pub struct MediaParam {
    pub availability_start_time: String, // RFC3339
    pub video: Option<Video>,
    pub audio: Option<Audio>,
}

pub struct Video {
    pub codec: String,     // avc1.640028
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub timescale: u32,
    pub bandwidth: u32,
}

pub struct Audio {
    pub codec: String,     // mp4a.40.2
    pub sample_rate: u32,
    pub channels: u32,
    pub timescale: u32,
    pub bandwidth: u32,
}
