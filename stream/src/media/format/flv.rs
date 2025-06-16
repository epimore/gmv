use std::ffi::{c_int, c_void, CString};
use std::ptr;
use crate::media::format::demuxer;
use crate::media::format::muxer::MuxerSink;
use common::bytes::Bytes;
use common::exception::GlobalResult;
use common::once_cell::sync::Lazy;
use common::tokio::sync::broadcast;
use rsmpeg::ffi::{av_guess_format, av_malloc, av_packet_ref, av_packet_unref, avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context, AVFormatContext, AVIOContext, AVPacket};

static FLV: Lazy<CString> = Lazy::new(|| CString::new("flv").unwrap());
pub struct FlvMuxer {
    pub flv_header: Bytes,
    //is_idr packet
    pub flv_body_tx: broadcast::Sender<(bool, Bytes)>,
    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    pub out_buf: Vec<u8>,
}
impl Drop for FlvMuxer {
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
impl FlvMuxer {
    pub fn new(flv_body_tx: broadcast::Sender<(bool, Bytes)>, demuxer_context: &demuxer::DemuxerContext) -> GlobalResult<Self> {
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

            let fmt_ctx = avformat_alloc_context();
            let flv_fmt = av_guess_format(FLV.as_ptr(), ptr::null(), ptr::null());
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = flv_fmt;

            for &codecpar in &demuxer_context.codecpar_list {
                let stream = avformat_new_stream(fmt_ctx, ptr::null_mut());
                avcodec_parameters_copy((*stream).codecpar, codecpar);
                (*stream).codecpar.codec_tag = 0;
            }

            // 写 header
            if avformat_write_header(fmt_ctx, std::ptr::null_mut()) < 0 {
                return Err("FLV header write failed".into());
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

    pub fn get_header(&self) -> Bytes {
        self.flv_header.clone()
    }

    unsafe extern "C" fn write_callback(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
        unsafe {
            let out_buffer = &mut *(opaque as *mut Vec<u8>);
            let data = std::slice::from_raw_parts(buf, buf_size as usize);
            out_buffer.extend_from_slice(data);
            buf_size
        }
    }
}

impl MuxerSink for FlvMuxer {
    fn write_packet(&self, pkt: &AVPacket) {
        unsafe {
            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);

            // 记录旧的 out_buf 长度
            let before_len = self.out_buf.len();

            // 写入 FLV packet 到缓冲区
            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);

            if ret < 0 {
                common::log::warn!("FLV write failed: {}", ret);
                return;
            }

            let after_len = self.out_buf.len();
            if after_len <= before_len {
                return;
            }

            // 获取新写入的字节（即这个 packet 的 FLV 表示）
            let packet_bytes = &self.out_buf[before_len..after_len];

            // 判断是否为视频关键帧
            let is_idr = pkt.stream_index == 0 && (pkt.flags & rsmpeg::ffi::AV_PKT_FLAG_KEY != 0);

            let _ = self
                .flv_body_tx
                .send((is_idr, Bytes::copy_from_slice(packet_bytes)));

            // 清理 out_buf，保留 header（可选）或直接清空
            self.out_buf.truncate(0); // 完全清空（推荐）
        }
    }
}
