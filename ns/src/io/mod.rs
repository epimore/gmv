use std::{ptr, slice};
use std::os::raw::{c_int, c_void};

use flume::{Receiver, RecvError, Sender};
use ffmpeg_next::ffi::AVERROR_EOF;
use common::bytes::Bytes;
use common::err::TransError;
use common::log::{debug, warn};

use crate::state::session;

mod input;
pub mod output;


type FuncReadPacket = unsafe extern fn(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int;

pub fn call_read() -> FuncReadPacket {
    read_packet
}

//_buf_size 由网络接口定义接收的数据大小 < buf_size , 故此处不再做判断
#[no_mangle]
unsafe extern "C" fn read_packet(opaque: *mut c_void, buf: *mut u8, _buf_size: c_int) -> c_int {
    let ssrc = &*(opaque as *const u32);
    match session::get_rtp_rx(ssrc) {
        None => {
            warn!("ssrc = {ssrc},流已释放");
            AVERROR_EOF
        }
        Some(rx) => {
            match rx.recv() {
                Ok(bytes) => {
                    debug!("---------buffer  = {:?}",&bytes);
                    let len = bytes.len();
                    let br = bytes.to_vec().as_ptr();
                    ptr::copy_nonoverlapping(br, buf, len);
                    // debug!("========= buf  = {:?}",Vec::from_raw_parts(buf, len, buffer.capacity()));
                    len as c_int
                }
                Err(err) => {
                    warn!("ssrc = {ssrc},获取流失败,err = {:?}",err);
                    AVERROR_EOF
                }
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn tx_flv_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let ssrc = &*(opaque as *const u32);
    match session::get_flv_tx(ssrc) {
        None => {
            warn!("ssrc = {ssrc},流已释放");
            -1
        }
        Some(flv_tx) => {
            match flv_tx {
                None => { -1 }
                Some(tx) => {
                    let slice = slice::from_raw_parts(buf, buf_size as usize);
                    let _ = tx.try_send(Bytes::copy_from_slice(slice)).hand_err(|msg| debug!("{msg}"));
                    buf_size
                }
            }
        }
    }
}

#[no_mangle]
unsafe extern "C" fn tx_m3u8_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let ssrc = &*(opaque as *const u32);
    match session::get_m3u8_tx(ssrc) {
        None => {
            warn!("ssrc = {ssrc},流已释放");
            -1
        }
        Some(m3u8_tx) => {
            match m3u8_tx {
                None => { -1 }
                Some(tx) => {
                    let slice = slice::from_raw_parts(buf, buf_size as usize);
                    let _ = tx.try_send(Bytes::copy_from_slice(slice)).hand_err(|msg| debug!("{msg}"));
                    buf_size
                }
            }
        }
    }
}