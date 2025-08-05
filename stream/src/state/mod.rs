pub mod cache;
pub mod msg;
pub mod layer;

//格式化通道大小
pub const FORMAT_BROADCAST_BUFFER: usize = 16;
//统一超时：毫秒
pub const TIME_OUT: u64 = 8000;
//默认流空闲关闭：毫秒
pub const STREAM_IDLE_TIME_OUT: u64 = 6000;
pub const HALF_TIME_OUT: u64 = 4000;
//数据通道缓存大小
pub const RTP_BUFFER_SIZE: usize = 64;
//API接口根信息
pub const INDEX: &str = r#"<!DOCTYPE html><html lang="en"><head>
    <style>body{display:grid;place-items:center;height:100vh;margin:0;}<bof/style>
    <metacharset="UTF - 8"><title>GMV</title></head>
<body><div><h1>GMV:STREAM-SERVER</h1></div></body></html>"#;