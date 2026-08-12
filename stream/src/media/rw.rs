use crate::media::rtp::{self, RtpReadOutcome};
use rsmpeg::ffi::{AVERROR_EOF, AVERROR_EXIT};
use std::ffi::{c_int, c_void};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_payload(
    opaque: *mut c_void,
    buf: *mut u8,
    buf_size: c_int,
) -> c_int {
    unsafe {
        let rtp_buffer = &mut *(opaque as *mut rtp::RtpPacketBuffer);
        loop {
            match rtp_buffer.consume_packet(buf_size as usize, buf) {
                RtpReadOutcome::Data(copy_len) => return copy_len as c_int,
                RtpReadOutcome::WouldBlock => continue,
                RtpReadOutcome::Interrupted(_) => return AVERROR_EXIT,
                RtpReadOutcome::Closed => return AVERROR_EOF,
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_rtp_read(opaque: *mut c_void) -> c_int {
    let control = unsafe { (opaque as *const rtp::RtpReadControl).as_ref() };
    i32::from(control.is_some_and(|control| control.interrupt_reason().is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::rtp::{RtpPacket, RtpPacketBuffer, RtpReadControl};
    use base::bytes::Bytes;
    use base::tokio_util::sync::CancellationToken;
    use crossbeam_channel::bounded;
    use gmv_domain::info::media_info_ext::MediaExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn read_callback_waits_through_temporary_input_gap() {
        let (tx, rx) = bounded(1);
        let control = Arc::new(RtpReadControl::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(1),
        ));
        let mut rtp_buffer = RtpPacketBuffer::init(200_000_177, rx, &MediaExt::default(), control);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            tx.send(RtpPacket {
                ssrc: 200_000_177,
                timestamp: 90_000,
                marker: true,
                seq: 1,
                payload: Bytes::from_static(b"frame"),
            })
            .unwrap();
        });
        let mut output = [0u8; 16];

        let read = unsafe {
            read_rtp_payload(
                (&mut rtp_buffer as *mut RtpPacketBuffer).cast::<c_void>(),
                output.as_mut_ptr(),
                output.len() as c_int,
            )
        };

        sender.join().unwrap();
        assert_eq!(read, 5);
        assert_eq!(&output[..read as usize], b"frame");
    }

    #[test]
    fn read_callback_returns_exit_when_cancelled() {
        let (_tx, rx) = bounded(1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let control = Arc::new(RtpReadControl::new(
            cancel,
            Instant::now() + Duration::from_secs(1),
        ));
        let mut rtp_buffer = RtpPacketBuffer::init(200_000_177, rx, &MediaExt::default(), control);
        let mut output = [0u8; 16];

        let read = unsafe {
            read_rtp_payload(
                (&mut rtp_buffer as *mut RtpPacketBuffer).cast::<c_void>(),
                output.as_mut_ptr(),
                output.len() as c_int,
            )
        };

        assert_eq!(read, AVERROR_EXIT);
    }
}
