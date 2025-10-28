use crate::media::context::MediaContext;
use crate::state::msg::StreamConfig;
use crate::state::{cache};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio;
use base::tokio::sync::mpsc::Receiver;
use rsmpeg::ffi::{av_log_set_level, av_strerror, AVPacket, AV_LOG_QUIET};
use std::ffi::c_int;
use std::sync::Arc;
use base::bytes::Bytes;
use base::tokio::sync::broadcast;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::MuxPacket;

mod rw;
pub mod rtp;
pub mod context;

pub const DEFAULT_IO_BUF_SIZE: usize = 32768;
//todo! 转发媒体流，不进入MediaContext
pub async fn handle_process(mut rx: Receiver<u32>) {
    unsafe {
        av_log_set_level(AV_LOG_QUIET);
    }
    while let Some(ssrc) = rx.recv().await {
        if let Ok(mut sc_rx) = cache::sub_bus_mpsc_channel::<StreamConfig>(&ssrc) {
            //此处可以不使用超时等待，统一流输入超时处理即可；输入超时-清理该ssrc所有信息，包含此处的发送句柄，完成资源释放
            if let Ok(stream_config) = sc_rx.recv().await.hand_log(|msg| error!("{}",msg)) {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = MediaContext::init(ssrc, stream_config).map(|mut ctx| ctx.invoke());
                });
            }
        }
    }
}

pub fn show_ffmpeg_error_msg(ret: c_int) -> String {
    let mut buf = [0u8; 1024];
    unsafe {
        av_strerror(ret, buf.as_mut_ptr() as *mut i8, buf.len());
        let cstr = std::ffi::CStr::from_ptr(buf.as_ptr() as *const i8);
        cstr.to_string_lossy().into_owned()
    }
}

pub trait DataWriter {
    fn init(demuxer_context: &DemuxerContext, pkt: broadcast::Sender<Arc<MuxPacket>>) -> GlobalResult<Self>
    where
        Self: Sized;
    fn get_header(&self) -> Bytes;
    fn write_body(&mut self, pkt: &AVPacket,timestamp: u64);
    fn get_trailer(&mut self);
}