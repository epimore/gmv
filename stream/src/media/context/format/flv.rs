use crate::media::context::format::demuxer::DemuxerContext;
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{info, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{
    av_guess_format, av_malloc, av_packet_ref, av_packet_unref, avcodec_parameters_copy,
    avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context,
    avio_context_free, avio_flush, AVFormatContext, AVIOContext, AVPacket, AVFMT_FLAG_FLUSH_PACKETS,
};
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::Arc;
use crate::media::{show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};

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
    out_buf_ptr: *mut Vec<u8>, // heap 上的输出缓冲
}

impl Drop for FlvContext {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                rsmpeg::ffi::avformat_free_context(self.fmt_ctx);
                self.fmt_ctx = ptr::null_mut();
            }
            if !self.avio_ctx.is_null() {
                avio_context_free(&mut self.avio_ctx);
                self.avio_ctx = ptr::null_mut();
            }
            // io_buf 由 avio_context_free 释放，这里不再单独 free
            self.io_buf = ptr::null_mut();

            if !self.out_buf_ptr.is_null() {
                drop(Box::from_raw(self.out_buf_ptr)); // 收回 heap 上的 Vec
                self.out_buf_ptr = ptr::null_mut();
            }
        }
    }
}

impl FlvContext {
    pub fn get_header(&self) -> Bytes {
        self.flv_header.clone()
    }

    unsafe extern "C" fn write_callback(
        opaque: *mut c_void,
        buf: *mut u8,
        buf_size: c_int,
    ) -> c_int {
        if opaque.is_null() || buf.is_null() || buf_size <= 0 {
            return buf_size;
        }
        let out_vec: &mut Vec<u8> = &mut *(opaque as *mut Vec<u8>);
        let old_len = out_vec.len();
        out_vec.reserve(buf_size as usize);
        std::ptr::copy_nonoverlapping(
            buf,
            out_vec.as_mut_ptr().add(old_len),
            buf_size as usize,
        );
        out_vec.set_len(old_len + buf_size as usize);
        buf_size
    }

    pub fn write_packet(&mut self, pkt: &AVPacket) {
        unsafe {
            if pkt.size == 0 || pkt.data.is_null() {
                warn!("Skipping empty or invalid packet");
                return;
            }

            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);

            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);

            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                warn!("FLV write failed: {}, error: {}", ret, ffmpeg_error);
                return;
            }

            // 强制刷新 IO
            avio_flush((*self.fmt_ctx).pb);

            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                warn!("FLV write failed: out buffer is empty");
                return;
            }

            let data = Bytes::from(out_vec.clone());
            out_vec.clear();

            let is_key = pkt.stream_index == 0
                && (pkt.flags & rsmpeg::ffi::AV_PKT_FLAG_KEY as i32 != 0);
            let _ = self.flv_body_tx.send(Arc::new(FlvPacket { data, is_key }));
        }
    }

    pub fn init_context(
        demuxer_context: &DemuxerContext,
        flv_body_tx: broadcast::Sender<Arc<FlvPacket>>,
    ) -> GlobalResult<Self> {
        unsafe {
            let io_buf_size = DEFAULT_IO_BUF_SIZE;
            let io_buf = av_malloc(io_buf_size) as *mut u8;
            if io_buf.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate IO buffer",
                    |msg| warn!("{msg}"),
                ));
            }

            // 输出缓冲放在 heap 上，保证地址稳定
            let out_box: Box<Vec<u8>> = Box::new(Vec::new());
            let out_buf_ptr: *mut Vec<u8> = Box::into_raw(out_box);

            let avio_ctx = avio_alloc_context(
                io_buf,
                io_buf_size as c_int,
                1,
                out_buf_ptr as *mut c_void,
                None,
                Some(Self::write_callback),
                None,
            );
            if avio_ctx.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate AVIO context",
                    |msg| warn!("{msg}"),
                ));
            }

            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = av_guess_format(FLV.as_ptr(), ptr::null(), ptr::null());
            (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            if (*fmt_ctx).oformat.is_null() {
                return Err(GlobalError::new_sys_error(
                    "FLV format not supported",
                    |msg| warn!("{msg}"),
                ));
            }

            if demuxer_context.codecpar_list.is_empty() {
                return Err(GlobalError::new_sys_error(
                    "No codec parameters available",
                    |msg| warn!("{msg}"),
                ));
            }

            for &codecpar in &demuxer_context.codecpar_list {
                let stream = avformat_new_stream(fmt_ctx, ptr::null_mut());
                if stream.is_null() {
                    return Err(GlobalError::new_sys_error(
                        "Failed to create stream",
                        |msg| warn!("{msg}"),
                    ));
                }
                let ret = avcodec_parameters_copy((*stream).codecpar, codecpar);
                if ret < 0 {
                    return Err(GlobalError::new_sys_error(
                        &format!("Codecpar copy failed: {}", ret),
                        |msg| warn!("{msg}"),
                    ));
                }
                (*(*stream).codecpar).codec_tag = 0;
            }

            if (*fmt_ctx).nb_streams == 0 {
                return Err(GlobalError::new_sys_error(
                    "No streams added to muxer",
                    |msg| warn!("{msg}"),
                ));
            }

            // 写入头部
            let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
            if ret < 0 {
                return Err(GlobalError::new_sys_error(
                    &format!("FLV header write failed: {}", show_ffmpeg_error_msg(ret)),
                    |msg| warn!("{msg}"),
                ));
            }

            // 从 out_buf 中取 header（克隆+清空）
            let out_vec = &mut *out_buf_ptr;
            let flv_header = Bytes::from(out_vec.clone());
            out_vec.clear();

            Ok(FlvContext {
                flv_header,
                flv_body_tx,
                fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf_ptr,
            })
        }
    }
}
