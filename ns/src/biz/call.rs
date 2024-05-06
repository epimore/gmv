use std::net::SocketAddr;
use constructor::New;

#[derive(New)]
pub struct RtpInfo {
    ssrc: u32,
    //tcp/udp
    protocol: String,
    //媒体流源地址
    origin_addr: String,
    server_name: String,
}

impl RtpInfo {
    //未知流，每隔8秒提示一次
    pub async fn stream_unknown(&self) {}
}

#[derive(New)]
pub struct BaseStreamInfo {
    rtp_info: RtpInfo,
    stream_id: String,
    in_time: u32,
}

impl BaseStreamInfo {
    //当接收到输入流时进行回调
    pub async fn stream_in(&self) {}
    //当流闲置时（无观看、无录制），依旧接收到ssrc流输入时，间隔8秒回调一次
    pub async fn stream_idle(&self) {}
}

pub struct StreamPlayInfo {
    base_stream_info: BaseStreamInfo,
    remote_addr: SocketAddr,
    token: String,
    //0-flv,1-hls
    play_type: u8,
    //当前观看人数
    hls_play_count: u32,
    flv_play_count: u32,
}

impl StreamPlayInfo {
    //当用户访问播放流时进行回调（可用于鉴权）
    pub async fn on_play(&self) {}

    //当用户断开播放时进行回调
    pub async fn off_play(&self) {}
}

pub struct StreamRecordInfo {
    base_stream_info: BaseStreamInfo,
    file_path: String,
    file_name: String,
    //单位kb
    file_size: u32,
}

impl StreamRecordInfo {
    //当流录制完成时进行回调
    pub async fn end_record(&self) {}
}

pub struct StreamState {
    base_stream_info: BaseStreamInfo,
    hls_play_count: u32,
    flv_play_count: u32,
    record_enable: Option<bool>,
}

impl StreamState {
    //当等待输入流超时时进行回调
    pub async fn stream_input_timeout(&self) {}
}