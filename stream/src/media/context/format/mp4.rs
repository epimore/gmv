use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{write_callback, FmtMuxer, MuxPacket};
use crate::media::{show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{av_free, av_guess_format, av_malloc, av_packet_ref, av_packet_rescale_ts, av_packet_unref, av_rescale_q, av_write_trailer, avcodec_parameters_copy, avformat_alloc_context, avformat_free_context, avformat_new_stream, avformat_write_header, avio_alloc_context, avio_context_free, av_interleaved_write_frame, AVFormatContext, AVIOContext, AVPacket, AVRational, AVFMT_FLAG_FLUSH_PACKETS, AV_PKT_FLAG_KEY, AVDictionary, av_dict_set, av_dict_free};
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
                avformat_free_context(self.fmt_ctx);
                self.fmt_ctx = ptr::null_mut();
            }
            if !self.avio_ctx.is_null() {
                // avio_context_free expects pointer-to-pointer
                avio_context_free(&mut self.avio_ctx);
                self.avio_ctx = ptr::null_mut();
            }
            // io_buf 由 avio_context_free 释放（如果 avio_ctx 已存在）
            self.io_buf = ptr::null_mut();

            if !self.out_buf_ptr.is_null() {
                // 回收 heap 上的 Vec<u8>
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
            // 分配 io buffer
            let io_buf_size = DEFAULT_IO_BUF_SIZE;
            let io_buf = av_malloc(io_buf_size) as *mut u8;
            if io_buf.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate IO buffer",
                    |msg| warn!("{msg}"),
                ));
            }

            // 准备 out_vec 的 Box，并取得裸指针
            let out_box: Box<Vec<u8>> = Box::new(Vec::new());
            let out_buf_ptr: *mut Vec<u8> = Box::into_raw(out_box);

            // avio_alloc_context expects u8* buffer; we're写操作(write_flag=1)
            let mut avio_ctx = avio_alloc_context(
                io_buf,
                io_buf_size as c_int,
                1,
                out_buf_ptr as *mut c_void,
                None,
                Some(write_callback),
                None,
            );
            if avio_ctx.is_null() {
                // avio_ctx 创建失败：需要手动释放 io_buf 与 out_buf_ptr
                av_free(io_buf as *mut c_void);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate AVIO context",
                    |msg| warn!("{msg}"),
                ));
            }

            // 分配 format context
            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                // 清理已创建的资源
                avio_context_free(&mut avio_ctx); // 正确传入可变引用
                // avio_context_free 会释放 io_buf（因为 avio_ctx 持有它）
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }

            // 绑定 avio 到 fmt_ctx
            (*fmt_ctx).pb = avio_ctx;
            let guessed = av_guess_format(MP4.as_ptr(), ptr::null(), ptr::null());
            if guessed.is_null() {
                // 如果找不到 MP4 输出格式，释放资源
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "MP4 format not supported",
                    |msg| warn!("{msg}"),
                ));
            }
            (*fmt_ctx).oformat = guessed;

            // MP4：可以保留 FLUSH_PACKETS 标志以保证实时写入小片段
            (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;

            // 检查 demuxer 的 codecpar 列表
            if demuxer_context.codecpar_list.is_empty() {
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "No codec parameters available",
                    |msg| warn!("{msg}"),
                ));
            }

            // 检查输入 fmt_ctx 是否可用
            let in_fmt = demuxer_context.avio.fmt_ctx;
            if in_fmt.is_null() {
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Input format context is null",
                    |msg| warn!("{msg}"),
                ));
            }

            let nb_in = (*in_fmt).nb_streams as usize;

            // 如果 codecpar_list 的长度大于输入流数，认为不合法
            if demuxer_context.codecpar_list.len() > nb_in {
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Codecpar list length exceeds input nb_streams",
                    |msg| warn!("{msg}"),
                ));
            }

            let mut in_tbs: Vec<AVRational> = Vec::with_capacity(demuxer_context.codecpar_list.len());
            let mut out_tbs: Vec<AVRational> = Vec::with_capacity(demuxer_context.codecpar_list.len());

            // 建立输出流，并对齐时基
            for i in 0..demuxer_context.codecpar_list.len() {
                let codecpar = demuxer_context.codecpar_list[i];

                // 读取输入流指针，确保在 nb_in 范围内（上面已检查）
                let in_st = *(*in_fmt).streams.offset(i as isize);
                let out_st = avformat_new_stream(fmt_ctx, ptr::null_mut());
                if out_st.is_null() {
                    avio_context_free(&mut avio_ctx);
                    avformat_free_context(fmt_ctx);
                    drop(Box::from_raw(out_buf_ptr));
                    return Err(GlobalError::new_sys_error(
                        "Failed to create stream",
                        |msg| warn!("{msg}"),
                    ));
                }

                let ret = avcodec_parameters_copy((*out_st).codecpar, codecpar);
                if ret < 0 {
                    avio_context_free(&mut avio_ctx);
                    avformat_free_context(fmt_ctx);
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
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "No streams added to muxer",
                    |msg| warn!("{msg}"),
                ));
            }

            let mut opts: *mut AVDictionary = ptr::null_mut();
            // movflags 用 fragmented mp4 以避免需要 seek（适合非 seek 的 avio 回调）
            let key = CString::new("movflags").unwrap();
            let val = CString::new("frag_keyframe+empty_moov+default_base_moof+faststart").unwrap();
            let set_ret = av_dict_set(&mut opts, key.as_ptr(), val.as_ptr(), 0);
            if set_ret < 0 {
                // 如果设置字典失败也不要 panic，继续但记录 warn
                warn!("Failed to set mp4 movflags dict: {}", set_ret);
            }

            let ret = unsafe { avformat_write_header(fmt_ctx, &mut opts) };
            if ret < 0 {
                // 写 header 失败：释放资源并返回错误
                av_dict_free(&mut opts);
                avio_context_free(&mut avio_ctx);
                avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    &format!("MP4 header write failed: {}", show_ffmpeg_error_msg(ret)),
                    |msg| warn!("{msg}"),
                ));
            }

            // 取出 header bytes（如果 avio 的 write callback 已向 out_vec 写入）
            let out_vec = &mut *out_buf_ptr;
            let header = if out_vec.is_empty() {
                Bytes::new()
            } else {
                let header_bytes = std::mem::replace(out_vec, Vec::new());
                Bytes::from(header_bytes)
            };
            av_dict_free(&mut opts);
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

            let ret = av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
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

            // determine keyframe (use cloned.flags 更可靠)
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
            if !self.fmt_ctx.is_null() {
                av_write_trailer(self.fmt_ctx);
            }
        }
    }
}
