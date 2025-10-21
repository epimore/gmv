use crate::media::context::format::demuxer::DemuxerContext;
use axum::body::Bytes;
use base::exception::GlobalResult;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::AVPacket;
use std::ffi::{c_int, c_void};
use std::sync::Arc;

pub mod cmaf;
pub mod demuxer;
pub mod flv;
mod hls_ts;
pub mod mp4;
pub mod muxer;
mod ps;
pub mod rtp;
pub mod ts;

pub struct MuxPacket {
    pub data: Bytes,
    pub is_key: bool,
    pub timestamp: u64,
}

pub trait FmtMuxer {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: broadcast::Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self>
    where
        Self: Sized;
    fn get_header(&self) -> Bytes;
    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64);
    fn flush(&mut self);
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
