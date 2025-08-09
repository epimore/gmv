use crate::media::rtp;
use base::log::{debug, warn};
use rsmpeg::ffi::AVERROR_EOF;
use std::ffi::{c_int, c_void};
use std::ptr;

pub struct SdpMemory {
    data: *const u8,
    len: usize,
    pos: usize,
}
impl SdpMemory {
    pub fn new(sdp: String) -> Self {
        let sdp_bytes = sdp.as_bytes();
        SdpMemory {
            data: sdp_bytes.as_ptr(),
            len: sdp_bytes.len(),
            pos: 0,
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_sdp_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int{
    let sdp = &mut *(opaque as *mut SdpMemory);
    let remaining = sdp.len.saturating_sub(sdp.pos);
    let read_len = buf_size.min(remaining as c_int);
    if read_len <= 0 {
        return AVERROR_EOF;
    }
    ptr::copy_nonoverlapping(sdp.data.add(sdp.pos), buf, read_len as usize);
    sdp.pos += read_len as usize;
    read_len
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_packet(opaque: *mut c_void, buf: *mut u8, _buf_size: c_int) -> c_int {
    let ibf = &mut *(opaque as *mut rtp::RtpPacketBuffer);
    match ibf.demux_packet() {
        Ok(None) => {
            warn!("Cache data buffer error: Short term no data, ffmpeg will retry");
            0
        }
        Ok(Some(data)) => {
            let len = data.len();
            let src = data.as_ptr();
            ptr::copy_nonoverlapping(src, buf, len);
            debug!("ffmpeg consumed packet len: {}", len);
            len as c_int
        }
        Err(err) => {
            debug!("rtp input stream close: {:?}", err);
            AVERROR_EOF
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn write_rtcp_packet(
    _opaque: *mut c_void,
    _buf: *mut u8,
    buf_size: c_int,
) -> c_int {
    debug!("ffmpeg produced rtcp packet len: {}", buf_size);
    buf_size
}