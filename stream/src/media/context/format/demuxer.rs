use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{
    av_dict_set,
    av_find_input_format,
    av_free,
    av_malloc,
    avcodec_parameters_alloc,
    avcodec_parameters_copy,
    avcodec_parameters_free,
    avformat_alloc_context,
    avformat_close_input,
    avformat_find_stream_info,
    avformat_free_context,
    avformat_open_input,
    avio_alloc_context,
    AVCodecParameters,
    AVDictionary,
    AVFormatContext,
    AVIOContext,
    AVMediaType_AVMEDIA_TYPE_VIDEO,
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
            }
            if !self.io_buf.is_null() {
                av_free(self.io_buf as *mut c_void);
            }
            if !self.avio_ctx.is_null() {
                av_free(self.avio_ctx as *mut c_void);
            }
        }
    }
}

#[derive(Clone)]
pub struct DemuxerContext {
    pub avio: Arc<AvioResource>,
    pub codecpar_list: Vec<*mut AVCodecParameters>,
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
            // Allocate format context
            let mut fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }

            // Set custom IO flags
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;

            // Set up format context options
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();

            // Configure format options for PS stream
            let fflags_key = CString::new("fflags").unwrap();
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts+igndts+sortdts").unwrap();
            av_dict_set(&mut dict_opts, fflags_key.as_ptr(), fflags_val.as_ptr(), 0);

            let strict_std_compliance_key = CString::new("strict").unwrap();
            let strict_std_compliance_val = CString::new("experimental").unwrap();
            av_dict_set(&mut dict_opts, strict_std_compliance_key.as_ptr(), strict_std_compliance_val.as_ptr(), 0);

            // 缩小探测窗口 —— 尽快返回流信息
            // 注：analyzeduration=0 在某些老版本会被当作“禁用分析”，若遇到打不开/识别差，可改成 50000（50ms）
            // let analyzeduration_key = CString::new("analyzeduration").unwrap();
            // let analyzeduration_val = CString::new("0").unwrap();
            // av_dict_set(&mut dict_opts, analyzeduration_key.as_ptr(), analyzeduration_val.as_ptr(), 0);

            let probesize_key = CString::new("probesize").unwrap();
            let probesize_val = CString::new("32768").unwrap(); // 32k，够快且相对稳
            av_dict_set(&mut dict_opts, probesize_key.as_ptr(), probesize_val.as_ptr(), 0);

            // 消除队头等待
            let max_delay_key = CString::new("max_delay").unwrap();
            let max_delay_val = CString::new("0").unwrap();
            av_dict_set(&mut dict_opts, max_delay_key.as_ptr(), max_delay_val.as_ptr(), 0);

            // Find the MPEG-PS demuxer
            let ps = CString::new("mpeg").unwrap(); // Using 'mpeg' instead of 'mpegps' for better compatibility
            let mut input_fmt = av_find_input_format(ps.as_ptr());
            if input_fmt.is_null() {
                warn!("MPEG-PS demuxer not found");
                input_fmt = av_find_input_format(ptr::null());
            }

            // Set up custom AVIO context
            let io_ctx_buffer = av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
            if io_ctx_buffer.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to allocate IO buffer", |msg| error!("{msg}")));
            }

            let rtp_buf_ptr = Box::into_raw(Box::new(rtp_buffer)) as *mut c_void;
            let io_ctx = avio_alloc_context(
                io_ctx_buffer,
                DEFAULT_IO_BUF_SIZE as c_int,
                0, // Write flag (0 for read-only)
                rtp_buf_ptr,
                Some(rw::read_rtp_payload),
                None, // No writing
                None, // No seeking
            );

            if io_ctx.is_null() {
                av_free(io_ctx_buffer as *mut c_void);
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| error!("{msg}")));
            }
            // 禁止 FFmpeg 尝试 seek（流式传输）
            (*io_ctx).seekable = 0;
            // Assign the AVIO context to the format context
            (*fmt_ctx).pb = io_ctx;

            // Open the input
            let input_url = ptr::null();
            let ret = avformat_open_input(
                &mut fmt_ctx,
                input_url,
                input_fmt,
                &mut dict_opts,
            );

            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                av_free(io_ctx as *mut c_void);
                av_free(io_ctx_buffer as *mut c_void);
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open input: {}", msg)));
            }

            // Find stream info
            let ret = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                avformat_close_input(&mut fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to find stream info: {}", msg)));
            }

            // Log format info
            let fmt_name = CStr::from_ptr((*(*fmt_ctx).iformat).name).to_string_lossy();
            info!("Input format: {}", fmt_name);

            // Process streams
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

                info!("Stream {}: type = {}, codec_id = {}, format = {}, width = {}, height = {}",
                    i,
                    (*codecpar).codec_type,
                    (*codecpar).codec_id,
                    (*codecpar).format,
                    (*codecpar).width,
                    (*codecpar).height);
                let extradata = (*codecpar).extradata;
                let extradata_size = (*codecpar).extradata_size;
                if extradata.is_null() || extradata_size <= 0 {
                    //return Err(GlobalError::new_biz_error(1100, "H.264 stream missing SPS/PPS data", |msg| error!("{msg}")));
                    warn!("H.264 stream missing SPS/PPS data");
                } else {
                    info!("H.264 extradata size: {}", extradata_size);
                }
                codecpar_list.push(codecpar);
                stream_mapping.push((i, is_video));
            }

            // Clean up dictionary
            rsmpeg::ffi::av_dict_free(&mut dict_opts);

            Ok(Self {
                avio: Arc::new(AvioResource {
                    fmt_ctx,
                    io_buf: io_ctx_buffer,
                    avio_ctx: io_ctx,
                }),
                codecpar_list,
                stream_mapping,
            })
        }
    }
}
