use std::ffi::c_int;
use std::sync::Arc;
use std::time::Duration;
use common::exception::{GlobalResult, TransError};
use common::log::error;
use common::tokio;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::{broadcast, Mutex};
use common::tokio::time::{timeout};
use rsmpeg::ffi::{av_strerror, avformat_network_init};
use crate::general::mode::HALF_TIME_OUT;
use crate::io::hook_handler::{InEvent, RtpStreamEvent};
use crate::state::cache;

mod rw;
mod rtp;
mod format;


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
async fn handle_run(mut rx: Receiver<u32>) {
    while let Some(ssrc) = rx.recv().await {
        match cache::get_in_event_shard_rx(&ssrc) {
            None => { error!("invalid ssrc {}", ssrc); }
            Some(in_event_rx) => {
                if check_stream_in(ssrc, in_event_rx).await.is_ok() {
                    if let Some((sdp_map, rtp_rx)) = cache::get_rx_sdp_tx(&ssrc) {
                        let _ = tokio::task::spawn_blocking(move || {
                            let rtp_packet_buffer = rtp::RtpPacketBuffer::init(ssrc, rtp_rx);
                            format::demuxer::start_demuxer(sdp_map, rtp_packet_buffer, av_tx);
                        }).await.hand_log(|msg| error!("{}",msg));
                    }
                }
            }
        }
    }
}

async fn check_stream_in(ssrc: u32, in_event_rx: Arc<Mutex<broadcast::Receiver<InEvent>>>) -> GlobalResult<()> {
    timeout(Duration::from_millis(HALF_TIME_OUT), async {
        loop {
            let in_event = in_event_rx.lock().await.recv().await.hand_log(|msg| error!("{msg}"))?;
            if let InEvent::RtpStreamEvent(RtpStreamEvent::StreamIn) = in_event {
                break;
            }
        };
    }).await.hand_log(|_| error!("ssrc = {} media stream in timeout",ssrc))?;
    Ok(())
}

pub fn show_ffmpeg_error_msg(ret: c_int) -> String {
    let mut buf = [0u8; 1024];
    unsafe {
        av_strerror(ret, buf.as_mut_ptr() as *mut i8, buf.len());
        let cstr = std::ffi::CStr::from_ptr(buf.as_ptr() as *const i8);
        cstr.to_string_lossy().into_owned()
    }
}