use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{
    av_dict_set, av_find_input_format, av_free, av_malloc, avcodec_parameters_alloc,
    avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input,
    avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context,
    avio_context_free, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_VIDEO,
    AVFMT_FLAG_CUSTOM_IO,
};
use shared::info::media_info_ext::MediaExt;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;

static CUSTOM_IO: Lazy<CString> = Lazy::new(|| CString::new("custom_io").unwrap());

/// FFmpeg资源自动释放结构
pub struct AvioResource {
    pub fmt_ctx: *mut AVFormatContext,
    pub io_buf: *mut u8,
    pub avio_ctx: *mut AVIOContext,
}
unsafe impl Send for AvioResource {}

impl Drop for AvioResource {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                avformat_close_input(&mut self.fmt_ctx);
                self.fmt_ctx = ptr::null_mut();
            }
            if !self.avio_ctx.is_null() {
                // 取出 opaque（rtp_buffer），随后统一回收
                let opaque = (*self.avio_ctx).opaque;
                // 正确释放 AVIOContext（会连带释放内部 buffer）
                avio_context_free(&mut self.avio_ctx);
                self.avio_ctx = ptr::null_mut();
                // io_buf 由 avio_context_free 释放，不要再手动 free
                self.io_buf = ptr::null_mut();
                // 回收 rtp_buffer（保证只在这一个地方回收）
                if !opaque.is_null() {
                    drop(Box::<rtp::RtpPacketBuffer>::from_raw(opaque as *mut _));
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct DemuxerContext {
    pub avio: Arc<AvioResource>,
    pub codecpar_list: Vec<*mut rsmpeg::ffi::AVCodecParameters>,
    pub stream_mapping: Vec<(usize, bool)>,
}
impl Drop for DemuxerContext {
    fn drop(&mut self) {
        unsafe {
            for &par in &self.codecpar_list {
                if !par.is_null() {
                    avcodec_parameters_free(&mut (par as *mut _));
                }
            }
        }
    }
}

impl DemuxerContext {
    pub fn start_demuxer(_ssrc: u32, _media_ext: &MediaExt, rtp_buffer: rtp::RtpPacketBuffer) -> GlobalResult<Self> {
        unsafe {
            // 1) alloc fmt_ctx
            let mut fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;

            // 2) dict
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();
            let fflags_key = CString::new("fflags").unwrap();
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts").unwrap();// 先去掉 sortdts/genpts，稳定性优先
            av_dict_set(&mut dict_opts, fflags_key.as_ptr(), fflags_val.as_ptr(), 0);

            let strict_std_compliance_key = CString::new("strict").unwrap();
            let strict_std_compliance_val = CString::new("experimental").unwrap();
            av_dict_set(&mut dict_opts, strict_std_compliance_key.as_ptr(), strict_std_compliance_val.as_ptr(), 0);

            let probesize_key = CString::new("probesize").unwrap();
            let probesize_val = CString::new("32768").unwrap(); // 32 KiB，PS over RTP 更稳
            av_dict_set(&mut dict_opts, probesize_key.as_ptr(), probesize_val.as_ptr(), 0);

            let max_delay_key = CString::new("max_delay").unwrap();
            let max_delay_val = CString::new("0").unwrap();
            av_dict_set(&mut dict_opts, max_delay_key.as_ptr(), max_delay_val.as_ptr(), 0);

            // 3) input fmt
            let ps = CString::new("mpeg").unwrap();
            let mut input_fmt = av_find_input_format(ps.as_ptr());
            if input_fmt.is_null() {
                warn!("MPEG-PS demuxer not found");
                input_fmt = av_find_input_format(ptr::null());
            }

            // 4) custom AVIO
            let io_ctx_buffer = av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
            if io_ctx_buffer.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to allocate IO buffer", |msg| error!("{msg}")));
            }

            let rtp_buf_ptr = Box::into_raw(Box::new(rtp_buffer)) as *mut c_void;

            let mut io_ctx = avio_alloc_context(
                io_ctx_buffer,
                DEFAULT_IO_BUF_SIZE as c_int,
                0,
                rtp_buf_ptr,
                Some(rw::read_rtp_payload),
                None,
                None,
            );
            if io_ctx.is_null() {
                av_free(io_ctx_buffer as *mut c_void);
                avformat_free_context(fmt_ctx);
                // 回收 opaque
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| error!("{msg}")));
            }
            (*io_ctx).seekable = 0;
            (*fmt_ctx).pb = io_ctx;

            // 5) open input
            let input_url = ptr::null();
            let ret = avformat_open_input(&mut fmt_ctx, input_url, input_fmt, &mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                // 正确释放：会释放 buffer
                avio_context_free(&mut io_ctx);
                // 回收 opaque
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open input: {}", msg)));
            }

            // 6) find stream info
            let ret = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                avformat_close_input(&mut fmt_ctx); // 会内部关闭 pb? 保险起见交由 AvioResource::drop 处理其余
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to find stream info: {}", msg)));
            }

            let fmt_name = CStr::from_ptr((*(*fmt_ctx).iformat).name).to_string_lossy();
            info!("Input format: {}", fmt_name);

            let nb_streams = (*fmt_ctx).nb_streams as usize;
            let mut codecpar_list = Vec::with_capacity(nb_streams);
            let mut stream_mapping = Vec::with_capacity(nb_streams);

            for i in 0..nb_streams {
                let stream = *(*fmt_ctx).streams.offset(i as isize);
                let codecpar = avcodec_parameters_alloc();
                if codecpar.is_null() {
                    avformat_close_input(&mut fmt_ctx);
                    return Err(GlobalError::new_sys_error("Failed to allocate codec parameters", |msg| error!("{msg}")));
                }
                avcodec_parameters_copy(codecpar, (*stream).codecpar);
                let is_video = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;

                info!(
                    "Stream {}: type = {}, codec_id = {}, format = {}, w = {}, h = {}",
                    i, (*codecpar).codec_type, (*codecpar).codec_id, (*codecpar).format, (*codecpar).width, (*codecpar).height
                );

                let extradata = (*codecpar).extradata;
                let extradata_size = (*codecpar).extradata_size;
                if extradata.is_null() || extradata_size <= 0 {
                    warn!("H.264 stream missing SPS/PPS data");
                } else {
                    info!("H.264 extradata size: {}", extradata_size);
                }
                codecpar_list.push(codecpar);
                stream_mapping.push((i, is_video));
            }

            rsmpeg::ffi::av_dict_free(&mut dict_opts);

            Ok(Self {
                avio: Arc::new(AvioResource {
                    fmt_ctx,
                    io_buf: io_ctx_buffer, // 仅保存在结构体里，释放由 avio_context_free 负责
                    avio_ctx: io_ctx,
                }),
                codecpar_list,
                stream_mapping,
            })
        }
    }
}
