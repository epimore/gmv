use crate::media::context::format::MuxPacket;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::{MediaCompletion, MediaContext, MediaRunError};
use crate::state::msg::StreamConfig;
use crate::state::register::Register;
use base::bytes::Bytes;
use base::exception::GlobalResult;
use base::log::{debug, error};
use base::tokio;
use base::tokio::sync::broadcast;
use base::tokio::sync::mpsc::Receiver;
use base::utils::rt::GlobalRuntime;
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
pub async fn handle_process(mut rx: Receiver<u32>, runtime: GlobalRuntime) {
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

    loop {
        let ssrc = base::tokio::select! {
            _ = runtime.cancel.cancelled() => break,
            ssrc = rx.recv() => {
                let Some(ssrc) = ssrc else {
                    if !runtime.cancel.is_cancelled() {
                        error!("media dispatcher input closed unexpectedly");
                        GlobalRuntime::request_shutdown_with_error();
                    }
                    break;
                };
                ssrc
            }
        };
        if let Ok(mut sc_rx) = Register::sub_bus_mpsc_channel::<StreamConfig>(&ssrc) {
            //此处可以不使用超时等待，统一流输入超时处理即可；输入超时-清理该ssrc所有信息，包含此处的发送句柄，完成资源释放
            let stream_config = base::tokio::select! {
                _ = runtime.cancel.cancelled() => break,
                stream_config = sc_rx.recv() => stream_config,
            };
            if let Ok(stream_config) = stream_config {
                let stream_id =
                    Register::stream_id_by_ssrc(ssrc).unwrap_or_else(|| Arc::from("unknown"));
                let worker_stream_id = stream_id.clone();
                let worker_runtime = runtime.clone();
                let task_name = format!("stream-media-worker-{ssrc}");
                if let Err(err) = runtime.spawn_blocking(task_name, move || {
                    let Ok((mut ctx, muxer_layer)) = MediaContext::init(ssrc, stream_config) else {
                        return;
                    };
                    let result = ctx.invoke(muxer_layer);
                    let failed = result.is_err();
                    match result {
                        Ok(MediaCompletion::Eof) => debug!(
                            "media worker completed: stage=demux, outcome=eof, stream_id={}, ssrc={}",
                            worker_stream_id, ssrc
                        ),
                        Ok(MediaCompletion::InputClosed) => debug!(
                            "media worker completed: stage=context, outcome=input_closed, stream_id={}, ssrc={}",
                            worker_stream_id, ssrc
                        ),
                        Err(MediaRunError::Ffmpeg {
                            stage,
                            code,
                            message,
                        }) => error!(
                            "media worker failed: stage={}, outcome=ffmpeg_error, stream_id={}, ssrc={}, ffmpeg_code={}, reason={}",
                            stage, worker_stream_id, ssrc, code, message
                        ),
                        Err(MediaRunError::Pipeline(_)) => {}
                    }
                    if failed && worker_runtime.is_shutting_down() {
                        GlobalRuntime::request_shutdown_with_error();
                    }
                }) {
                    error!(
                        "media worker spawn failed: stage=spawn, outcome=runtime_rejected, stream_id={}, ssrc={}, reason={err}",
                        stream_id, ssrc
                    );
                }
            } else {
                debug!(
                    "media worker setup ended: stage=stream_config, outcome=input_closed, ssrc={}",
                    ssrc
                );
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
