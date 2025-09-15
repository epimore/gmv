use crate::media::context::format::demuxer::DemuxerContext;
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, info, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{av_guess_format, av_malloc, av_packet_ref, av_packet_unref, avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context, avio_context_free, avio_flush, AVFormatContext, AVIOContext, AVPacket, AVFMT_FLAG_FLUSH_PACKETS, AVRational, AVMediaType_AVMEDIA_TYPE_VIDEO, AV_PKT_FLAG_KEY, av_free, avformat_alloc_output_context2, av_dump_format, avcodec_parameters_from_context, av_packet_rescale_ts, av_rescale_q};
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::Arc;
use crate::media::{show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use crate::media::context::format::{write_callback, MuxPacket};

static FLV: Lazy<CString> = Lazy::new(|| CString::new("flv").unwrap());

pub struct FlvContext {
    pub flv_header: Bytes,
    pub flv_body_tx: broadcast::Sender<Arc<MuxPacket>>,
    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    // 新增：时基与视频流索引、起播控制
    in_time_bases: Vec<AVRational>,
    out_time_bases: Vec<AVRational>,
    video_stream_index: i32,
    started: bool,
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
            // io_buf 由 avio_context_free 释放
            self.io_buf = ptr::null_mut();

            if !self.out_buf_ptr.is_null() {
                drop(Box::from_raw(self.out_buf_ptr));
                self.out_buf_ptr = ptr::null_mut();
            }
        }
    }
}

impl FlvContext {
    pub fn get_header(&self) -> Bytes {
        self.flv_header.clone()
    }

    pub fn write_packet(&mut self, pkt: &AVPacket,timestamp: u64) {
        unsafe {
            if pkt.size == 0 || pkt.data.is_null() {
                warn!("Skipping empty or invalid packet");
                return;
            }

            // 克隆
            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);

            let si = pkt.stream_index as usize;
            if si >= self.in_time_bases.len() || si >= self.out_time_bases.len() {
                av_packet_unref(&mut cloned);
                warn!("stream_index out of range: {}", si);
                return;
            }

            // 关键帧起播：先等视频关键帧
            if !self.started {
                if self.video_stream_index >= 0 && pkt.stream_index == self.video_stream_index {
                    let is_key = (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0;
                    if !is_key {
                        av_packet_unref(&mut cloned);
                        return;
                    }
                    self.started = true;
                } else {
                    // 未开始时，非视频流先不发，避免卡在等关键帧
                    av_packet_unref(&mut cloned);
                    return;
                }
            }

            debug!("FLV write_packet before rescale: stream={} cloned.pts={} cloned.dts={} cloned.duration={} in_tb={}/{} out_tb={}/{}",
    si,
    cloned.pts,
    cloned.dts,
    cloned.duration,
    self.in_time_bases[si].num, self.in_time_bases[si].den,
    self.out_time_bases[si].num, self.out_time_bases[si].den,
);

            // 时间戳重采样
            let orig_duration = pkt.duration;
            av_packet_rescale_ts(
                &mut cloned,
                self.in_time_bases[si],
                self.out_time_bases[si],
            );
            if orig_duration > 0 {
                cloned.duration = av_rescale_q(
                    orig_duration,
                    self.in_time_bases[si],
                    self.out_time_bases[si],
                );
            }
            debug!("FLV write_packet after rescale: stream={} cloned.pts={} cloned.dts={} cloned.duration={}",
    si,
    cloned.pts,
    cloned.dts,
    cloned.duration,
);

            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                warn!("FLV write failed: {}, error: {}", ret, ffmpeg_error);
                return;
            }

            // 刷新 IO
            // avio_flush((*self.fmt_ctx).pb);

            // 取出产出数据
            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                return;
            }
            let data = Bytes::from(out_vec.clone());
            out_vec.clear();

            let is_key_out = self.video_stream_index >= 0
                && pkt.stream_index == self.video_stream_index
                && (pkt.flags & AV_PKT_FLAG_KEY as i32 != 0);

            let _ = self.flv_body_tx.send(Arc::new(MuxPacket { data, is_key: is_key_out, timestamp }));
        }
    }

    pub fn init_context(
        demuxer_context: &DemuxerContext,
        flv_body_tx: broadcast::Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self> {
        unsafe {
            let io_buf_size = DEFAULT_IO_BUF_SIZE;
            let io_buf = av_malloc(io_buf_size) as *mut u8;
            if io_buf.is_null() {
                return Err(GlobalError::new_sys_error("Failed to allocate IO buffer", |msg| warn!("{msg}")));
            }

            let out_box: Box<Vec<u8>> = Box::new(Vec::new());
            let out_buf_ptr: *mut Vec<u8> = Box::into_raw(out_box);

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
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| warn!("{msg}")));
            }

            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                avio_context_free(&mut (avio_ctx.clone()));
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| warn!("{msg}")));
            }
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = av_guess_format(FLV.as_ptr(), ptr::null(), ptr::null());
            (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            if (*fmt_ctx).oformat.is_null() {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error("FLV format not supported", |msg| warn!("{msg}")));
            }

            if demuxer_context.codecpar_list.is_empty() {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error("No codec parameters available", |msg| warn!("{msg}")));
            }

            let in_fmt = demuxer_context.avio.fmt_ctx;
            let nb_in = (*in_fmt).nb_streams as usize;

            let mut in_tbs: Vec<AVRational> = Vec::with_capacity(nb_in);
            let mut out_tbs: Vec<AVRational> = Vec::with_capacity(nb_in);
            let mut video_si: i32 = -1;

            // 建立输出流，并对齐时基
            for i in 0..demuxer_context.codecpar_list.len() {
                let codecpar = demuxer_context.codecpar_list[i];

                let in_st = *(*in_fmt).streams.offset(i as isize);
                let out_st = avformat_new_stream(fmt_ctx, ptr::null_mut());
                if out_st.is_null() {
                    avio_context_free(&mut (avio_ctx.clone()));
                    rsmpeg::ffi::avformat_free_context(fmt_ctx);
                    drop(Box::from_raw(out_buf_ptr));
                    return Err(GlobalError::new_sys_error("Failed to create stream", |msg| warn!("{msg}")));
                }

                let ret = avcodec_parameters_copy((*out_st).codecpar, codecpar);
                if ret < 0 {
                    avio_context_free(&mut (avio_ctx.clone()));
                    rsmpeg::ffi::avformat_free_context(fmt_ctx);
                    drop(Box::from_raw(out_buf_ptr));
                    return Err(GlobalError::new_sys_error(&format!("Codecpar copy failed: {}", ret), |msg| warn!("{msg}")));
                }

                // 根据流类型设置FLV适当的时间基
                let out_time_base = match (*(*out_st).codecpar).codec_type {
                    AVMediaType_AVMEDIA_TYPE_VIDEO => {
                        // FLV视频时间基：1/1000 (毫秒)
                        AVRational { num: 1, den: 1000 }
                    }
                    AVMediaType_AVMEDIA_TYPE_AUDIO => {
                        // FLV音频时间基：1/采样率
                        let sample_rate = (*(*out_st).codecpar).sample_rate.max(1);
                        AVRational { num: 1, den: sample_rate }
                    }
                    _ => (*in_st).time_base // 其他流保持原样
                };

                (*out_st).time_base = out_time_base;

                if (*(*out_st).codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO && video_si < 0 {
                    video_si = i as i32;
                }

                in_tbs.push((*in_st).time_base);
                out_tbs.push(out_time_base); // 使用设置好的输出时间基
                (*(*out_st).codecpar).codec_tag = 0;
            }

            if (*fmt_ctx).nb_streams == 0 {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error("No streams added to muxer", |msg| warn!("{msg}")));
            }

            // av_dump_format(fmt_ctx, 0, FLV.as_ptr(), 1);

            let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
            if ret < 0 {
                avio_context_free(&mut (avio_ctx.clone()));
                rsmpeg::ffi::avformat_free_context(fmt_ctx);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(&format!("FLV header write failed: {}", show_ffmpeg_error_msg(ret)), |msg| warn!("{msg}")));
            }
            // for (i, tb) in in_tbs.iter().enumerate() {
            //     info!("FLV init: in_time_base[{}] = {}/{}", i, tb.num, tb.den);
            // }
            // for (i, tb) in out_tbs.iter().enumerate() {
            //     info!("FLV init: out_time_base[{}] = {}/{}", i, tb.num, tb.den);
            // }
            // info!("FLV video_stream_index = {}", video_si);

            let out_vec = &mut *out_buf_ptr;
            let flv_header = Bytes::from(std::mem::replace(out_vec, Vec::new()));
            // out_vec.clear();

            Ok(FlvContext {
                flv_header,
                flv_body_tx,
                fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf_ptr,
                in_time_bases: in_tbs,
                out_time_bases: out_tbs,
                video_stream_index: video_si,
                started: false,
            })
        }
    }
}
