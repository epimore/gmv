use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{write_callback, FmtMuxer, MuxPacket};
use crate::media::{show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{
    av_free, av_guess_format, av_malloc, av_packet_ref,
    av_packet_rescale_ts, av_packet_unref, av_rescale_q, av_write_trailer, avcodec_parameters_copy, avformat_alloc_context,
    avformat_new_stream, avformat_write_header, avio_alloc_context, avio_context_free, AVFormatContext,
    AVIOContext, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational,
    AVFMT_FLAG_FLUSH_PACKETS, AV_PKT_FLAG_KEY,
};
use std::ffi::{c_int, c_void, CString};
use std::os::raw::c_uchar;
use std::ptr;
use std::sync::Arc;

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());

pub struct Mp4Context {
    pub header: Bytes,
    pub pkt_tx: broadcast::Sender<Arc<MuxPacket>>,
    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,
    in_time_bases: Vec<AVRational>,
    out_time_bases: Vec<AVRational>,
}

impl Drop for Mp4Context {
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
            // io_buf 由 avio_context_free 释放
            self.io_buf = ptr::null_mut();

            if !self.out_buf_ptr.is_null() {
                drop(Box::from_raw(self.out_buf_ptr));
                self.out_buf_ptr = ptr::null_mut();
            }
        }
    }
}

impl FmtMuxer for Mp4Context {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: broadcast::Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self>
    where
        Self: Sized,
    {
        unsafe {
            let io_buf_size = DEFAULT_IO_BUF_SIZE;
            let io_buf = av_malloc(io_buf_size) as *mut u8;
            if io_buf.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate IO buffer",
                    |msg| warn!("{msg}"),
                ));
            }

            let out_box: Box<Vec<u8>> = Box::new(Vec::new());
            let out_buf_ptr: *mut Vec<u8> = Box::into_raw(out_box);

            // avio_alloc_context expects u8* buffer; we're writing, so write_flag=1
            let avio_ctx = avio_alloc_context(
                io_buf,
                io_buf_size as c_int,
                1,
                out_buf_ptr as *mut c_void,
                None,
                Some(write_callback),
                None,
            );
            if avio_ctx.is_null() {
                // io_buf 由 avio_context_free 释放；但这里 avio_ctx 创建失败，只能手动 free
                av_free(io_buf as *mut c_void);
                // 回收 out_buf_ptr
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate AVIO context",
                    |msg| warn!("{msg}"),
                ));
            }

            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                avio_context_free(&mut (avio_ctx.clone()));
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }

            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = av_guess_format(MP4.as_ptr(), ptr::null(), ptr::null());
            // MP4：可以保留 FLUSH_PACKETS 标志以保证实时写入小片段
            (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            if (*fmt_ctx).oformat.is_null() {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "MP4 format not supported",
                    |msg| warn!("{msg}"),
                ));
            }

            if demuxer_context.codecpar_list.is_empty() {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "No codec parameters available",
                    |msg| warn!("{msg}"),
                ));
            }

            let in_fmt = demuxer_context.avio.fmt_ctx;
            let nb_in = (*in_fmt).nb_streams as usize;

            let mut in_tbs: Vec<AVRational> = Vec::with_capacity(nb_in);
            let mut out_tbs: Vec<AVRational> = Vec::with_capacity(nb_in);

            // 建立输出流，并对齐时基
            for i in 0..demuxer_context.codecpar_list.len() {
                let codecpar = demuxer_context.codecpar_list[i];

                let in_st = *(*in_fmt).streams.offset(i as isize);
                let out_st = avformat_new_stream(fmt_ctx, ptr::null_mut());
                if out_st.is_null() {
                    avio_context_free(&mut (avio_ctx.clone()));
                    rsmpeg::ffi::avformat_free_context(fmt_ctx);
                    drop(Box::from_raw(out_buf_ptr));
                    return Err(GlobalError::new_sys_error(
                        "Failed to create stream",
                        |msg| warn!("{msg}"),
                    ));
                }

                let ret = avcodec_parameters_copy((*out_st).codecpar, codecpar);
                if ret < 0 {
                    avio_context_free(&mut (avio_ctx.clone()));
                    rsmpeg::ffi::avformat_free_context(fmt_ctx);
                    drop(Box::from_raw(out_buf_ptr));
                    return Err(GlobalError::new_sys_error(
                        &format!("Codecpar copy failed: {}", ret),
                        |msg| warn!("{msg}"),
                    ));
                }

                // 对于 MP4，通常使用输入流的 time_base 或 codec 推荐的 time_base
                let out_time_base = (*in_st).time_base;
                (*out_st).time_base = out_time_base;

                in_tbs.push((*in_st).time_base);
                out_tbs.push(out_time_base);
                (*(*out_st).codecpar).codec_tag = 0;
            }

            if (*fmt_ctx).nb_streams == 0 {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "No streams added to muxer",
                    |msg| warn!("{msg}"),
                ));
            }

            let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
            if ret < 0 {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    &format!("MP4 header write failed: {}", show_ffmpeg_error_msg(ret)),
                    |msg| warn!("{msg}"),
                ));
            }

            let out_vec = &mut *out_buf_ptr;
            let header_bytes = Bytes::from(std::mem::replace(out_vec, Vec::new()));
            let header = Bytes::from(header_bytes);
            Ok(Mp4Context {
                header,
                pkt_tx,
                fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf_ptr,
                in_time_bases: in_tbs,
                out_time_bases: out_tbs,
            })
        }
    }

    fn get_header(&self) -> Bytes {
        self.header.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) {
        unsafe {
            if pkt.size == 0 || pkt.data.is_null() {
                warn!("Skipping empty or invalid packet");
                return;
            }

            // clone packet
            let mut cloned = std::mem::zeroed::<AVPacket>();
            if av_packet_ref(&mut cloned, pkt) < 0 {
                warn!("Failed to ref packet");
                return;
            }

            let si = pkt.stream_index as usize;
            if si >= self.in_time_bases.len() || si >= self.out_time_bases.len() {
                av_packet_unref(&mut cloned);
                warn!("stream_index out of range: {}", si);
                return;
            }

            debug!(
                "MP4 write_packet before rescale: stream={} cloned.pts={} cloned.dts={} cloned.duration={} in_tb={}/{} out_tb={}/{}",
                si,
                cloned.pts,
                cloned.dts,
                cloned.duration,
                self.in_time_bases[si].num,
                self.in_time_bases[si].den,
                self.out_time_bases[si].num,
                self.out_time_bases[si].den,
            );

            // rescale timestamps
            let orig_duration = pkt.duration;
            av_packet_rescale_ts(&mut cloned, self.in_time_bases[si], self.out_time_bases[si]);
            if orig_duration > 0 {
                cloned.duration = av_rescale_q(
                    orig_duration,
                    self.in_time_bases[si],
                    self.out_time_bases[si],
                );
            }

            debug!(
                "MP4 write_packet after rescale: stream={} cloned.pts={} cloned.dts={} cloned.duration={}",
                si, cloned.pts, cloned.dts, cloned.duration,
            );

            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                warn!("MP4 write failed: {}, error: {}", ret, ffmpeg_error);
                return;
            }

            // pull produced data
            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                return;
            }
            let data_base = Bytes::from(out_vec.clone());
            out_vec.clear();

            // determine keyframe (if video)
            let is_key_out = (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0;

            let mux_packet = MuxPacket {
                data: data_base,
                is_key: is_key_out,
                timestamp,
            };

            let _ = self.pkt_tx.send(Arc::new(mux_packet));
        }
    }

    fn flush(&mut self) {
        unsafe {
            av_write_trailer(self.fmt_ctx);
        }
    }
}
