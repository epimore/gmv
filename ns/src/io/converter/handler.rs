use std::os::raw::{c_int, c_void};
use std::{ptr, slice};
use ffmpeg_next::ffi::AVERROR_EOF;
use common::bytes::Bytes;
use common::err::TransError;
use common::log::{debug, warn};
use crate::state::cache;

type FuncReadPacket = unsafe extern fn(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int;
type FuncWritePacket = unsafe extern fn(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int;

pub fn call_rtp_read() -> FuncReadPacket {
    read_rtp_packet
}

pub fn call_flv_write() -> FuncWritePacket {
    tx_flv_packet
}

pub fn call_hls_write() -> FuncWritePacket {
    tx_hls_packet
}

//_buf_size 由网络接口定义接收的数据大小 < buf_size , 故此处不再做判断
#[no_mangle]
unsafe extern "C" fn read_rtp_packet(opaque: *mut c_void, buf: *mut u8, _buf_size: c_int) -> c_int {
    let ssrc = &*(opaque as *const u32);
    match cache::get_rtp_rx(ssrc) {
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
    match cache::get_flv_tx(ssrc) {
        None => {
            warn!("ssrc = {ssrc},流已释放");
            -1
        }
        Some(flv_tx) => {
            let slice = slice::from_raw_parts(buf, buf_size as usize);
            println!("flv data size = {}", slice.len());
            let _ = flv_tx.send(Bytes::copy_from_slice(slice)).hand_err(|msg| debug!("flv 发送失败{msg}"));
            buf_size
        }
    }
}

#[no_mangle]
unsafe extern "C" fn tx_hls_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let ssrc = &*(opaque as *const u32);
    match cache::get_hls_tx(ssrc) {
        None => {
            warn!("ssrc = {ssrc},流已释放");
            -1
        }
        Some(hls_tx) => {
            let slice = slice::from_raw_parts(buf, buf_size as usize);
            let _ = hls_tx.send(Bytes::copy_from_slice(slice)).hand_err(|msg| debug!("hls 发送失败: {msg}"));
            buf_size
        }
    }
}