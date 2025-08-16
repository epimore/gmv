use crate::general::util;
use crate::media::rtp;
use base::log::{debug, info, warn};
use rsmpeg::ffi::AVERROR_EOF;
use std::ffi::{c_int, c_void};
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_rtp_payload(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let rtp_buff = &mut *(opaque as *mut rtp::RtpPacketBuffer);
    match rtp_buff.consume_packet(buf_size as usize, buf) {
        Ok(copy_len) => {
            // let buf_size_usize = buf_size as usize;
            // // 限制打印长度：取64和实际buf_size的最小值，避免越界
            // let print_len = std::cmp::min(64, buf_size_usize);
            // let mut hex_str = String::with_capacity(print_len * 3); // 预分配空间
            // 
            // for i in 0..print_len {
            //     // 安全读取：仅访问已确认有效的范围
            //     let data = buf.offset(i as isize).read(); // 解引用指针获取数据
            //     hex_str.push_str(&format!("{:02x} ", data)); // 格式化为两位十六进制
            // }
            // 
            // // 使用日志宏而非print!，支持日志级别控制
            // println!(
            //         "RTP payload (first {} bytes, total copied: {}): {}",
            //         print_len, copy_len, hex_str.trim_end()
            //     );
            copy_len as c_int
        }
        Err(err) => {
            info!("rtp input stream close: {:?}", err);
            AVERROR_EOF
        }
    }
}