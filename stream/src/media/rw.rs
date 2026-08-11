use crate::media::rtp::{self, RtpReadOutcome};
use rsmpeg::ffi::{AVERROR, AVERROR_EOF, AVERROR_EXIT, EAGAIN};
use std::ffi::{c_int, c_void};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_payload(
    opaque: *mut c_void,
    buf: *mut u8,
    buf_size: c_int,
) -> c_int {
    unsafe {
        let rtp_buffer = &mut *(opaque as *mut rtp::RtpPacketBuffer);
        match rtp_buffer.consume_packet(buf_size as usize, buf) {
            RtpReadOutcome::Data(copy_len) => copy_len as c_int,
            RtpReadOutcome::WouldBlock => AVERROR(EAGAIN),
            RtpReadOutcome::Interrupted(_) => AVERROR_EXIT,
            RtpReadOutcome::Closed => AVERROR_EOF,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_rtp_read(opaque: *mut c_void) -> c_int {
    let control = unsafe { (opaque as *const rtp::RtpReadControl).as_ref() };
    i32::from(control.is_some_and(|control| control.interrupt_reason().is_some()))
}
