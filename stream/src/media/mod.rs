use std::ffi::c_int;
use std::time::Duration;
use common::exception::{GlobalResult, GlobalResultExt};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc::Receiver;
use rsmpeg::ffi::{av_strerror, avformat_network_init};
use crate::media::context::MediaContext;
use crate::state::{cache, TIME_OUT};
use crate::state::msg::StreamConfig;

mod rw;
pub mod rtp;
mod msg;
pub(crate) mod context;

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
                unsafe { avformat_network_init() };
                handle_run(rx)
            });
    });
}


// fn build_muxer(media_out_sender: MuxerSender) -> (MuxerSender, fn(&DemuxerContext, MuxerSender) -> GlobalResult<MuxerEnum>) {
//     match media_out_sender {
//         MuxerSender::Flv(tx) => {
//             (Box::new(tx),
//              |ctx: &DemuxerContext, tx| {
//                  format::flv::FlvMuxer::init_muxer(ctx, tx)
//              })
//         }
//         _ => { unimplemented!() }
//     }
// }

async fn handle_run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        if let Ok(mut sc_rx) = cache::sub_bus_mpsc_channel::<StreamConfig>(&ssrc) {
            //此处可以不使用超时等待，统一流输入超时处理即可；输入超时-清理该ssrc所有信息，包含此处的发送句柄，完成资源释放
            if let Ok(stream_config) = sc_rx.recv().await.hand_log(|msg| error!("{}",msg)) {
                // if let Ok(stream_config) = sc_rx.recv_with_timeout(Duration::from_millis(TIME_OUT)).await.hand_log(|msg| error!("{}",msg)) {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = MediaContext::init(ssrc, stream_config).map(|ctx| ctx.invoke());
                }).await.hand_log(|msg| error!("{}",msg));
            }
        }
    }
}

// async fn check_stream_in(ssrc: u32, in_event_rx: Arc<Mutex<broadcast::Receiver<InEvent>>>) -> GlobalResult<()> {
//     timeout(Duration::from_millis(HALF_TIME_OUT), async {
//         loop {
//             let in_event = in_event_rx.lock().await.recv().await.hand_log(|msg| error!("{msg}"))?;
//             if let InEvent::RtpStreamEvent(RtpStreamEvent::StreamIn) = in_event {
//                 break;
//             }
//         };
//     }).await.hand_log(|_| error!("ssrc = {} media stream in timeout",ssrc))?;
//     Ok(())
// }

pub fn show_ffmpeg_error_msg(ret: c_int) -> String {
    let mut buf = [0u8; 1024];
    unsafe {
        av_strerror(ret, buf.as_mut_ptr() as *mut i8, buf.len());
        let cstr = std::ffi::CStr::from_ptr(buf.as_ptr() as *const i8);
        cstr.to_string_lossy().into_owned()
    }
}