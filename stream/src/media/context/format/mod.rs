use crate::media::context::format::demuxer::DemuxerContext;
use axum::body::Bytes;
use base::exception::GlobalResult;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::AVPacket;
use std::ffi::{c_int, c_void};
use std::sync::Arc;
use std::time::Instant;

pub mod dashmp4;
pub mod demuxer;
pub mod flv;
pub mod fmp4;
pub mod h265flv;
mod hls_ts;
pub mod hlsfmp4;
pub mod mp4;
pub mod muxer;
mod ps;
pub mod rtp;
pub mod ts;

pub struct MuxPacket {
    pub data: Bytes,
    pub is_key: bool,
    pub timestamp: u64,
    pub epoch: Instant,
    pub seq: usize,
}

pub trait FmtMuxer {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: broadcast::Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self>
    where
        Self: Sized;
    fn get_header(&self) -> Bytes;
    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()>;
    fn flush(&mut self);
}

pub(super) fn can_start_fragmented_output(
    started: bool,
    video_stream_index: c_int,
    packet_stream_index: c_int,
    is_keyframe: bool,
) -> bool {
    started || video_stream_index < 0 || (packet_stream_index == video_stream_index && is_keyframe)
}

pub unsafe extern "C" fn write_callback(
    opaque: *mut c_void,
    buf: *mut u8,
    buf_size: c_int,
) -> c_int {
    unsafe {
        if opaque.is_null() || buf.is_null() || buf_size <= 0 {
            return buf_size;
        }
        let out_vec: &mut Vec<u8> = &mut *(opaque as *mut Vec<u8>);
        let old_len = out_vec.len();
        out_vec.reserve(buf_size as usize);
        std::ptr::copy_nonoverlapping(buf, out_vec.as_mut_ptr().add(old_len), buf_size as usize);
        out_vec.set_len(old_len + buf_size as usize);
        buf_size
    }
}

#[cfg(test)]
mod tests {
    use super::can_start_fragmented_output;

    #[test]
    fn fragmented_video_output_waits_for_its_first_keyframe() {
        assert!(!can_start_fragmented_output(false, 0, 1, false));
        assert!(!can_start_fragmented_output(false, 0, 0, false));
        assert!(can_start_fragmented_output(false, 0, 0, true));
        assert!(can_start_fragmented_output(true, 0, 1, false));
    }

    #[test]
    fn fragmented_audio_only_output_can_start_immediately() {
        assert!(can_start_fragmented_output(false, -1, 0, false));
    }
}
