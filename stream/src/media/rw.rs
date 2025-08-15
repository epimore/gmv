use crate::general::util;
use crate::media::rtp;
use base::log::{debug, info, warn};
use rsmpeg::ffi::AVERROR_EOF;
use std::ffi::{c_int, c_void};
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_payload(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let ibf = &mut *(opaque as *mut rtp::RtpPacketBuffer);
    match ibf.demux_packet() {
        Ok(None) => {
            info!("Cache data buffer error: Short term no data, ffmpeg will retry");
            0
        }
        Ok(Some(data)) => {
            // let _ = util::dump("rtp_ps", &data, false);
            let data_len = data.len();
            let copy_len = data_len.min(buf_size as usize);
            let src = data.as_ptr();
            ptr::copy_nonoverlapping(src, buf, copy_len);
            // 如果数据未完全复制，需要缓存剩余部分
            if data_len > buf_size as usize {
                ibf.cache_remaining_data(&data[copy_len..]);
            }
            copy_len as c_int
        }
        Err(err) => {
            info!("rtp input stream close: {:?}", err);
            AVERROR_EOF
        }
    }
}