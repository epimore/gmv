use std::ffi::{c_int, c_void};
use axum::body::Bytes;

pub mod demuxer;
pub mod muxer;
pub mod flv;
pub mod mp4;
pub mod ts;
pub mod rtp;
pub mod cmaf;
mod ps;
mod hls_ts;

pub struct MuxPacket {
    pub data: Bytes,
    pub is_key: bool,
    pub timestamp: u64,
}

pub unsafe extern "C" fn write_callback(
    opaque: *mut c_void,
    buf: *mut u8,
    buf_size: c_int,
) -> c_int { unsafe {
    if opaque.is_null() || buf.is_null() || buf_size <= 0 {
        return buf_size;
    }
    let out_vec: &mut Vec<u8> = &mut *(opaque as *mut Vec<u8>);
    let old_len = out_vec.len();
    out_vec.reserve(buf_size as usize);
    std::ptr::copy_nonoverlapping(buf, out_vec.as_mut_ptr().add(old_len), buf_size as usize);
    out_vec.set_len(old_len + buf_size as usize);
    buf_size
}}