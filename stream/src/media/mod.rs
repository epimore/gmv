use crate::media::context::MediaContext;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::state::msg::StreamConfig;
use crate::state::register::Register;
use base::bytes::Bytes;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio;
use base::tokio::sync::broadcast;
use base::tokio::sync::mpsc::Receiver;
use log::LevelFilter;
use rsmpeg::ffi::{
    AV_LOG_DEBUG, AV_LOG_ERROR, AV_LOG_FATAL, AV_LOG_INFO, AV_LOG_QUIET, AV_LOG_WARNING, AVPacket,
    av_log_set_level, av_strerror,
};
use std::ffi::c_int;
use std::sync::Arc;

pub mod context;
pub mod rtp;
mod rw;

pub const DEFAULT_IO_BUF_SIZE: usize = 1024 * 1024;
// 转发媒体流，不进入MediaContext
pub async fn handle_process(mut rx: Receiver<u32>) {
    unsafe {
        let ff_level = match log::max_level() {
            LevelFilter::Off | LevelFilter::Error | LevelFilter::Warn | LevelFilter::Info => {
                AV_LOG_FATAL
            }
            LevelFilter::Debug => AV_LOG_WARNING,
            LevelFilter::Trace => AV_LOG_DEBUG,
        };
        av_log_set_level(ff_level as c_int);
    }

    while let Some(ssrc) = rx.recv().await {
        if let Ok(mut sc_rx) = Register::sub_bus_mpsc_channel::<StreamConfig>(&ssrc) {
            //此处可以不使用超时等待，统一流输入超时处理即可；输入超时-清理该ssrc所有信息，包含此处的发送句柄，完成资源释放
            if let Ok(stream_config) = sc_rx.recv().await.hand_log(|msg| error!("{}", msg)) {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = MediaContext::init(ssrc, stream_config)
                        .map(|(mut ctx, muxer_layer)| ctx.invoke(muxer_layer));
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
    fn init(
        demuxer_context: &DemuxerContext,
        pkt: broadcast::Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self>
    where
        Self: Sized;
    fn get_header(&self) -> Bytes;
    fn write_body(&mut self, pkt: &AVPacket, timestamp: u64);
    fn get_trailer(&mut self);
}
