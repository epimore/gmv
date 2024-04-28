use std::net::SocketAddr;

pub struct BaseStreamInfo {
    ssrc: String,
    stream_id: String,
    //tcp/udp
    protocol: Option<String>,
    in_time: Option<u32>,
    server_id: String,
}

impl BaseStreamInfo {
    //当接收到输入流时进行回调
    pub async fn stream_listen_in(&self) {}
    //当流无操作时（无观看、无录制），依旧接收到ssrc流输入时，间隔8秒回调一次
    pub async fn stream_none_opt(&self) {}
}

pub struct StreamPlayInfo {
    base_stream_info: BaseStreamInfo,
    remote_addr: SocketAddr,
    token: String,
    //0-flv,1-hls
    play_type: u8,
}

impl StreamPlayInfo {
    //当用户访问播放流时进行回调（可用于鉴权）
    pub async fn stream_on_play(&self) {}

    //当用户断开播放时进行回调
    pub async fn stream_off_play(&self) {}
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
    pub async fn stream_end_record(&self) {}
}

pub struct StreamRunEvent {
    base_stream_info: BaseStreamInfo,
    hls_enable: Option<bool>,
    flv_enable: Option<bool>,
    record_enable: Option<bool>,
}

impl StreamRunEvent {
    //当等待输入流超时时进行回调
    pub async fn stream_input_timeout(&self) {}
}