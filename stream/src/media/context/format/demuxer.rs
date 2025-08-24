use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{av_dict_set, av_find_input_format, av_free, av_malloc, avcodec_parameters_alloc, avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context, avio_context_free, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_VIDEO, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_NOFILLIN};
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
    pub fn start_demuxer(_ssrc: u32, media_ext: &MediaExt, rtp_buffer: rtp::RtpPacketBuffer) -> GlobalResult<Self> {
        unsafe {
            // 1) alloc fmt_ctx
            let mut fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;

            // 2) 设置 AVFormatContext 直接参数
            // 根据 media_ext 中的信息设置探测参数
            // 如果没有明确配置，使用合理的默认值
            (*fmt_ctx).probesize = 1024 * 100; // 默认 100KB 探测大小
            (*fmt_ctx).max_analyze_duration = 2 * 1000 * 1000; // 默认 2秒分析时长

            // 3) 设置 AVDictionary 选项
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();

            // 基本 flags
            let fflags_key = CString::new("fflags").unwrap();
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts").unwrap();
            av_dict_set(&mut dict_opts, fflags_key.as_ptr(), fflags_val.as_ptr(), 0);

            let strict_std_compliance_key = CString::new("strict").unwrap();
            let strict_std_compliance_val = CString::new("experimental").unwrap();
            av_dict_set(&mut dict_opts, strict_std_compliance_key.as_ptr(), strict_std_compliance_val.as_ptr(), 0);

            // 解码错误检测设置 - 遇到错误时跳过帧
            let decode_error_detection_key = CString::new("decode_error_detection").unwrap();
            let decode_error_detection_val = CString::new("skip_frame").unwrap();
            av_dict_set(&mut dict_opts, decode_error_detection_key.as_ptr(), decode_error_detection_val.as_ptr(), 0);

            // 根据视频参数设置编解码器相关选项
            if let Some(codec_id) = &media_ext.video_params.codec_id {
                let codec_key = CString::new("vcodec").unwrap();
                let codec_id = CString::new(codec_id.as_str()).unwrap();
                av_dict_set(&mut dict_opts, codec_key.as_ptr(), codec_id.as_ptr(), 0);
            }

            // 设置分辨率（如果已知）
            if let Some((width, height)) = media_ext.video_params.resolution {
                let width_key = CString::new("video_size").unwrap();
                let video_size_val = CString::new(format!("{}x{}", width, height)).unwrap();
                av_dict_set(&mut dict_opts, width_key.as_ptr(), video_size_val.as_ptr(), 0);
            }

            // 设置帧率（如果已知）
            if let Some(fps) = media_ext.video_params.fps {
                let fps_key = CString::new("framerate").unwrap();
                let fr = CString::new(fps.to_string()).unwrap();
                av_dict_set(&mut dict_opts, fps_key.as_ptr(), fr.as_ptr(), 0);
            }

            // 设置码率（如果已知）
            if let Some(bitrate) = media_ext.video_params.bitrate {
                let bitrate_key = CString::new("b:v").unwrap();
                let br = CString::new(bitrate.to_string()).unwrap();
                av_dict_set(&mut dict_opts, bitrate_key.as_ptr(), br.as_ptr(), 0);
            }

            // 音频参数设置
            if let Some(codec_id) = &media_ext.audio_params.codec_id {
                let acodec_key = CString::new("acodec").unwrap();
                let acodec = CString::new(codec_id.as_str()).unwrap();
                av_dict_set(&mut dict_opts, acodec_key.as_ptr(), acodec.as_ptr(), 0);
            }

            if let Some(sample_rate) = &media_ext.audio_params.sample_rate {
                let sample_rate_key = CString::new("sample_rate").unwrap();
                let sr = CString::new(sample_rate.as_str()).unwrap();
                av_dict_set(&mut dict_opts, sample_rate_key.as_ptr(), sr.as_ptr(), 0);
            }

            if let Some(codec_id) = &media_ext.audio_params.bitrate {
                let acodec_key = CString::new("b:a").unwrap();
                let acodec = CString::new(codec_id.as_str()).unwrap();
                av_dict_set(&mut dict_opts, acodec_key.as_ptr(), acodec.as_ptr(), 0);
            }
            // RTP 相关设置
            // if let Some(rtp_encrypt) = &media_ext.rtp_encrypt {
            //     // 这里可以根据 RTP 加密配置设置相应的参数
            //     info!("RTP encryption configured");
            // }

            // 设置 payload type
            // let payload_type_key = CString::new("payload_type").unwrap();
            // let pt = CString::new(media_ext.type_code.to_string()).unwrap();
            // av_dict_set(&mut dict_opts, payload_type_key.as_ptr(), pt.as_ptr(), 0);

            // 设置时钟频率
            let clock_rate_key = CString::new("clock_rate").unwrap();
            let cr = CString::new(media_ext.clock_rate.to_string()).unwrap();
            av_dict_set(&mut dict_opts, clock_rate_key.as_ptr(), cr.as_ptr(), 0);

            // 对于 GB28181 流编号
            // if let Some(stream_number) = media_ext.stream_number {
            //     let stream_num_key = CString::new("stream_number").unwrap();
            //     let sn = CString::new(stream_number.to_string()).unwrap();
            //     av_dict_set(&mut dict_opts, stream_num_key.as_ptr(), sn.as_ptr(), 0);
            // }

            // 4) input fmt - 根据媒体类型选择合适的格式
            let format_name = match media_ext.type_name.as_str() {
                "PS" => "mpeg",
                "H264" => "h264",
                "H265" => "hevc",
                "AAC" => "aac",
                "G711" => "pcm_alaw",
                _ => "mpeg", // 默认使用 MPEG-PS
            };

            let ps = CString::new(format_name).unwrap();
            let mut input_fmt = av_find_input_format(ps.as_ptr());
            if input_fmt.is_null() {
                warn!("{} demuxer not found, trying default", format_name);
                input_fmt = av_find_input_format(ptr::null());
            }

            // 5) custom AVIO
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
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| error!("{msg}")));
            }
            (*io_ctx).seekable = 0;
            (*fmt_ctx).pb = io_ctx;

            // 6) open input
            let input_url = ptr::null();
            let ret = avformat_open_input(&mut fmt_ctx, input_url, input_fmt, &mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                avio_context_free(&mut io_ctx);
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open input: {}", msg)));
            }

            // 7) find stream info
            let ret = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                avformat_close_input(&mut fmt_ctx);
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
