use crate::media::context::MediaContext;
use crate::state::msg::StreamConfig;
use crate::state::{cache};
use base::exception::GlobalResultExt;
use base::log::error;
use base::tokio;
use base::tokio::sync::mpsc::Receiver;
use rsmpeg::ffi::{av_log_set_level, av_strerror, avformat_network_init, AV_LOG_DEBUG};
use std::ffi::c_int;

mod rw;
pub mod rtp;
pub mod context;

pub fn build_worker_run(rx: Receiver<u32>) {
    std::thread::spawn(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .max_blocking_threads(1024)
            .enable_all()
            .thread_name("media-worker")
            .build()
            .hand_log(|msg| error!("{}",msg))
            .unwrap()
            .block_on({
                unsafe {
                    avformat_network_init();
                    // av_log_set_level(AV_LOG_DEBUG as c_int);
                };

                handle_run(rx)
            });
    });
}
//todo! 转发媒体流，不进入MediaContext
async fn handle_run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        if let Ok(mut sc_rx) = cache::sub_bus_mpsc_channel::<StreamConfig>(&ssrc) {
            //此处可以不使用超时等待，统一流输入超时处理即可；输入超时-清理该ssrc所有信息，包含此处的发送句柄，完成资源释放
            if let Ok(stream_config) = sc_rx.recv().await.hand_log(|msg| error!("{}",msg)) {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = MediaContext::init(ssrc, stream_config).map(|mut ctx| ctx.invoke());
                }).await.hand_log(|msg| error!("{}",msg));
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