use crate::media::context::format::demuxer::DemuxerContext;
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::warn;
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{av_guess_format, av_malloc, av_packet_ref, av_packet_unref, avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context, AVFormatContext, AVIOContext, AVPacket};
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::Arc;

static FLV: Lazy<CString> = Lazy::new(|| CString::new("flv").unwrap());


pub struct FlvPacket {
    pub data: Bytes,
    pub is_key: bool,
}

pub struct FlvContext {
    pub flv_header: Bytes,
    pub flv_body_tx: broadcast::Sender<Arc<FlvPacket>>,
    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    pub out_buf: Vec<u8>,
}
impl Drop for FlvContext {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                rsmpeg::ffi::avformat_free_context(self.fmt_ctx);
            }
            if !self.avio_ctx.is_null() {
                rsmpeg::ffi::av_free(self.avio_ctx as *mut c_void);
            }
            if !self.io_buf.is_null() {
                rsmpeg::ffi::av_free(self.io_buf as *mut c_void);
            }
        }
    }
}
impl FlvContext {
    pub fn get_header(&self) -> Bytes {
        self.flv_header.clone()
    }

    unsafe extern "C" fn write_callback(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
        let out_buffer = unsafe { &mut *(opaque as *mut Vec<u8>) };
        let available = out_buffer.len();
        if available == 0 {
            return 0;
        }
        let write_len = std::cmp::min(buf_size as usize, available);
        unsafe {
            std::ptr::copy_nonoverlapping(out_buffer.as_ptr(), buf, write_len);
        }
        out_buffer.drain(..write_len);
        write_len as c_int
    }

    pub fn write_packet(&mut self, pkt: &AVPacket) {
        unsafe {
            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);

            // 写入 FLV packet 到缓冲区
            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);

            if ret < 0 {
                warn!("FLV write failed: {}", ret);
                return;
            }
            if self.out_buf.is_empty() {
                warn!("FLV write failed: out buffer is empty");
                return;
            }

            let out_data = std::mem::take(&mut self.out_buf);

            // 判断是否为视频关键帧
            let is_key = pkt.stream_index == 0 && (pkt.flags & rsmpeg::ffi::AV_PKT_FLAG_KEY as i32 != 0);
            println!("is_key: {}", is_key);
            let _ = self
                .flv_body_tx
                .send(Arc::new(FlvPacket {
                    data: Bytes::from(out_data),
                    is_key,
                }));
        }
    }

    pub fn init_context(demuxer_context: &DemuxerContext, flv_body_tx: broadcast::Sender<Arc<FlvPacket>>) -> GlobalResult<Self> {
        unsafe {
            let io_buf_size = 4096;
            let io_buf = av_malloc(io_buf_size) as *mut u8;
            let mut buffer: Vec<u8> = Vec::new();
            let avio_ctx = avio_alloc_context(
                io_buf,
                io_buf_size as c_int,
                1,
                &mut buffer as *mut _ as *mut _,
                None,
                Some(Self::write_callback),
                None,
            );

            // 1. 验证FLV格式支持
            let flv_fmt = av_guess_format(FLV.as_ptr(), ptr::null(), ptr::null());
            if flv_fmt.is_null() {
                return Err(GlobalError::new_sys_error("FLV format not supported", |msg| warn!("{msg}")));
            }

            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| warn!("{msg}")));
            }
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = flv_fmt;

            if demuxer_context.codecpar_list.is_empty() {
                return Err(GlobalError::new_sys_error("No codec parameters available", |msg| warn!("{msg}")));
            }

            for &codecpar in &demuxer_context.codecpar_list {
                let stream = avformat_new_stream(fmt_ctx, ptr::null_mut());
                if stream.is_null() {
                    return Err(GlobalError::new_sys_error("Failed to create stream", |msg| warn!("{msg}")));
                }
                let ret = avcodec_parameters_copy((*stream).codecpar, codecpar);
                if ret < 0 {
                    return Err(GlobalError::new_sys_error(&format!("Codecpar copy failed: {}", ret), |msg| warn!("{msg}")));
                }
                (*(*stream).codecpar).codec_tag = 0;
            }
            if (*fmt_ctx).nb_streams == 0 {
                return Err(GlobalError::new_sys_error("No streams added to muxer", |msg| warn!("{msg}")));
            }
            // 写 header
            if avformat_write_header(fmt_ctx, std::ptr::null_mut()) < 0 {
                return Err(GlobalError::new_sys_error("FLV header write failed", |msg| warn!("{msg}")));
            }

            let flv_muxer = Self {
                flv_header: Bytes::copy_from_slice(&buffer),
                flv_body_tx,
                fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf: buffer,
            };
            Ok(flv_muxer)
        }
    }
}