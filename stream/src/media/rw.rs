use crate::media::rtp;
use base::log::{debug, info, warn};
use rsmpeg::ffi::AVERROR_EOF;
use std::ffi::{c_int, c_void};
use std::ptr;

pub struct SdpMemory {
    data: Vec<u8>,
    len: usize,
    pos: usize,
}
impl SdpMemory {
    pub fn new(sdp: String) -> Self {
        let data = sdp.into_bytes();
        let len = data.len();
        SdpMemory { data, len, pos: 0 }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_sdp_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let sdp = &mut *(opaque as *mut SdpMemory);
    let remaining = sdp.len.saturating_sub(sdp.pos);
    let read_len = (buf_size as usize).min(remaining) as c_int;
    if read_len <= 0 {
        return AVERROR_EOF;
    }
    ptr::copy_nonoverlapping(sdp.data.as_ptr().add(sdp.pos), buf, read_len as usize);
    sdp.pos += read_len as usize;
    read_len
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_packet(opaque: *mut c_void, buf: *mut u8, _buf_size: c_int) -> c_int {
    let ibf = &mut *(opaque as *mut rtp::RtpPacketBuffer);
    match ibf.demux_packet() {
        Ok(None) => {
            info!("Cache data buffer error: Short term no data, ffmpeg will retry");
            0
        }
        Ok(Some(data)) => {
            let len = data.len();
            let src = data.as_ptr();
            ptr::copy_nonoverlapping(src, buf, len);
            // info!("ffmpeg consumed packet len: {}", len);
            len as c_int
        }
        Err(err) => {
            info!("rtp input stream close: {:?}", err);
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