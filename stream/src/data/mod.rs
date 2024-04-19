use std::{ptr, slice};
use std::os::raw::{c_int, c_void};

use ffmpeg_next::ffi::AVERROR_EOF;
use ffmpeg_next::ffi::AVERROR_UNKNOWN;
use ffmpeg_next::sys::AVERROR_STREAM_NOT_FOUND;

use common::err::GlobalResult;
use common::log::{debug, warn};

use crate::data;

pub mod buffer;
pub mod session;

type FuncReadPacket = unsafe extern fn(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int;

pub fn call() -> FuncReadPacket {
    read_packet
}

//_buf_size 由网络接口定义了大小，此处不考虑
#[no_mangle]
unsafe extern "C" fn read_packet(opaque: *mut c_void, buf: *mut u8, _buf_size: c_int) -> c_int {
    let op = &*(opaque as *const u32);
    match buffer::Cache::consume(op) {
        Ok(None) => {
            // warn!("ssrc = {op},无数据");
            // AVERROR_UNKNOWN
            ffmpeg_next::ffi::EAGAIN
        }
        Ok(Some(mut buffer)) => {
            debug!("---------buffer  = {:?}",&buffer);
            let len = buffer.len();
            let cap = buffer.capacity();
            let br = buffer.as_mut_ptr();
            ptr::copy(br, buf, len);
            // debug!("========= buf  = {:?}",Vec::from_raw_parts(buf, len, cap));
            len as c_int
        }
        Err(err) => {
            warn!("ssrc = {op},流已释放");
            AVERROR_EOF
        }
    }
}

