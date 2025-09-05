use crate::media::rtp;
use base::log::info;
use rsmpeg::ffi::{AVERROR_EOF};
use std::ffi::{c_int, c_void};
use crate::media::context::RtpState;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_payload(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let (rtp_buff, rtp_state_ptr) = &mut *(opaque as *mut (rtp::RtpPacketBuffer, *mut RtpState));
    match rtp_buff.consume_packet(buf_size as usize, buf, *rtp_state_ptr) {
        Ok(copy_len) => {
            copy_len as c_int
        }
        Err(err) => {
            info!("rtp input stream close: {:?}", err);
            AVERROR_EOF
        }
    }
}
