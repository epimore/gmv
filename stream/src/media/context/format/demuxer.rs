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
//AVFormatContext *fmt_ctx = avformat_alloc_context();
//
// // 添加一个视频流
// AVStream *st = avformat_new_stream(fmt_ctx, NULL);
// AVCodecParameters *par = st->codecpar;
//
// par->codec_type = AVMEDIA_TYPE_VIDEO;
// par->codec_id   = AV_CODEC_ID_H264;
// par->width      = 1920;
// par->height     = 1080;
// par->format     = AV_PIX_FMT_YUV420P;
// par->extradata  = av_malloc(extradata_size + AV_INPUT_BUFFER_PADDING_SIZE);
// memcpy(par->extradata, extradata, extradata_size);
// par->extradata_size = extradata_size;
//
// // 设置时基（例：90k时钟）
// st->time_base = (AVRational){1, 90000};
//
// // 不调用 avformat_find_stream_info，直接用
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
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts").unwrap(); // 先去掉 sortdts/genpts，稳定性优先
            av_dict_set(&mut dict_opts, fflags_key.as_ptr(), fflags_val.as_ptr(), 0);

            let strict_std_compliance_key = CString::new("strict").unwrap();
            let strict_std_compliance_val = CString::new("experimental").unwrap();
            av_dict_set(&mut dict_opts, strict_std_compliance_key.as_ptr(), strict_std_compliance_val.as_ptr(), 0);

            // 缩小探测窗口 —— 尽快返回流信息
            // 注：analyzeduration=0 在某些老版本会被当作“禁用分析”，若遇到打不开/识别差，可改成 50000（50ms）
            let analyzeduration_key = CString::new("analyzeduration").unwrap();
            let analyzeduration_val = CString::new("2000000").unwrap();
            av_dict_set(&mut dict_opts, analyzeduration_key.as_ptr(), analyzeduration_val.as_ptr(), 0);

            let probesize_key = CString::new("probesize").unwrap();
            let probesize_val = CString::new("10240").unwrap(); // 32 KiB: 32768，PS over RTP 更稳
            av_dict_set(&mut dict_opts, probesize_key.as_ptr(), probesize_val.as_ptr(), 0);

            let max_delay_key = CString::new("max_delay").unwrap();
            let max_delay_val = CString::new("0").unwrap();
            av_dict_set(&mut dict_opts, max_delay_key.as_ptr(), max_delay_val.as_ptr(), 0);

            //// --- 限制探测的参数 ---
            // // 1. 限制探测的数据量大小 (单位：字节)
            // fmt_ctx->probesize = 2 * 1024; // 例如，只探测 2KB 数据
            // 
            // // 2. 限制探测的时长 (单位：微秒)
            // fmt_ctx->max_analyze_duration = 500000; // 例如，只分析最多0.5秒的数据
            // 
            // // 3. 设置标志位，避免昂贵的操作
            // // 不查找帧率（对于一些格式，查找帧率需要解码很多帧）
            // fmt_ctx->flags |= AVFMT_FLAG_NOFILLIN;
            // // 不生成缺失的PTS/DTS
            // fmt_ctx->flags |= AVFMT_FLAG_IGNIDX;
            // // 如果可能，快速探测
            // // 注意：这个标志并非所有格式都支持，但值得一试
            // fmt_ctx->flags |= AVFMT_FLAG_FAST_SEEK;
            //// 对于视频流，限制解码的帧数
            // av_dict_set(&opts, "decode_error_detection", "skip_frame", 0);
            // // 对于你的编码格式，可以尝试设置更具体的选项
            // // av_dict_set_int(&opts, "probesize", fmt_ctx->probesize); // 通常不需要，上面设置了
            //probesize：对于 ES 流，可以设置得相对较小。例如，一个 H.264 流的 SPS/PPS 通常就在最开始的几个包里，32KB 可能都绰绰有余。
            // 
            // max_analyze_duration：这个参数对 ES 流尤其重要。对于视频 ES 流，FFmpeg 可能会尝试解码直到遇到一个关键帧。如果文件开头没有关键帧，它可能会一直读下去。强烈建议设置一个绝对值（如 500000，即 0.5 秒）。
            // 
            // 选项字典 (opts)：使用 "decode_error_detection"="skip_frame" 可以让解码器在遇到错误时跳过而不是纠结，从而加快探测。
            // 不需要探测时长
            //用无缓冲标志
            // ：设置
            // AVFMT_FLAG_NOBUFFER标志可禁用FFmpeg内部缓冲机制，使数据直接从输入流
            // 传递至解码器，减少中间环节的等待时间。该标志尤其适用于实时流场景，能有效降低因缓冲堆积导
            // 致的阻塞概率


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


#[cfg(test)]
mod tests {
    use std::ffi::CStr;
    use std::ptr;
    use rsmpeg::ffi::{av_demuxer_iterate, av_opt_next, AVOption};
    use rsmpeg::ffi;

    #[test]

    fn for_supported_demuxer() {
        unsafe {
            let mut opaque = ptr::null_mut();
            while let Some(fmt) = av_demuxer_iterate(&mut opaque).as_ref() {
                let fmt_name = CStr::from_ptr((*fmt).name).to_string_lossy();
                let fmt_long_name = CStr::from_ptr((*fmt).long_name).to_string_lossy();
                println!("Supported demuxer: {}, {}", fmt_name, fmt_long_name);
            }
        }
    }

    #[test]
    fn for_enum_protocols() {
        unsafe {
            let mut opaque = ptr::null_mut();
            println!("Input protocols:");
            while let Some(protocol) = rsmpeg::ffi::avio_enum_protocols(&mut opaque, 0).as_ref() {
                let protocol_name = CStr::from_ptr(protocol).to_string_lossy();
                println!("  - {}", protocol_name);
            }
            println!("\nOutput protocols:");
            let mut opaque = ptr::null_mut();
            while let Some(protocol) = rsmpeg::ffi::avio_enum_protocols(&mut opaque, 1).as_ref() {
                let protocol_name = CStr::from_ptr(protocol).to_string_lossy();
                println!("  - {}", protocol_name);
            }
        }
    }
    #[test]
    fn dump_avoptions_for_format_context() {
        unsafe {
            let fmt_ctx = rsmpeg::ffi::avformat_alloc_context();
            let mut opt: *const rsmpeg::ffi::AVOption = std::ptr::null();
            let obj = fmt_ctx as *mut std::ffi::c_void;

            while {
                opt = rsmpeg::ffi::av_opt_next(obj, opt);
                !opt.is_null()
            } {
                let o = &*opt;
                let name = std::ffi::CStr::from_ptr(o.name).to_string_lossy();
                let help = if !o.help.is_null() {
                    std::ffi::CStr::from_ptr(o.help).to_string_lossy().into_owned()
                } else {
                    "".to_string()
                };
                println!(
                    "option: {} (help: {}, type: {}, min: {}, max: {})",
                    name, help, o.type_, o.min, o.max
                );
            }

            rsmpeg::ffi::avformat_free_context(fmt_ctx);
        }
    }


    /// 打印所有可用 demuxer 及其支持的参数
    #[test]
    fn dump_all_demuxer_options() {
        unsafe {
            let mut opaque: *mut std::ffi::c_void = ptr::null_mut();

            loop {
                let ifmt = av_demuxer_iterate(&mut opaque);
                if ifmt.is_null() {
                    break;
                }
                let name = if !(*ifmt).name.is_null() {
                    CStr::from_ptr((*ifmt).name).to_string_lossy().into_owned()
                } else {
                    "<unknown>".to_string()
                };

                println!("Demuxer: {}", name);

                let av_class = (*ifmt).priv_class;
                if !av_class.is_null() {
                    let mut opt: *const AVOption = ptr::null();
                    loop {
                        opt = av_opt_next(ptr::null(), opt);
                        if opt.is_null() {
                            break;
                        }
                        let opt_name = if !(*opt).name.is_null() {
                            CStr::from_ptr((*opt).name).to_string_lossy()
                        } else {
                            std::borrow::Cow::Borrowed("<noname>")
                        };
                        println!("    option: {}", opt_name);
                    }
                }
            }
        }
    }
}
