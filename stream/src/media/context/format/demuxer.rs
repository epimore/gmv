use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info, warn};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{av_dict_set, av_find_input_format, av_free, av_malloc, avcodec_parameters_alloc, avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context, avio_context_free, AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_ADPCM_G722, AVCodecID_AV_CODEC_ID_G723_1, AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H263, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_MPEG4, AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_UNKNOWN, AVMediaType_AVMEDIA_TYPE_VIDEO, AVRational, AVStream, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_NOFILLIN};
use shared::info::media_info_ext::MediaExt;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;

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


// --- 辅助：把字符串 codec 映射到 AVCodecID ---
unsafe fn map_video_codec_id(s: &str) -> AVCodecID {
    match s {
        "h264" => AVCodecID_AV_CODEC_ID_H264,
        "h265" | "hevc" => AVCodecID_AV_CODEC_ID_HEVC,
        "mpeg4" => AVCodecID_AV_CODEC_ID_MPEG4,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        "3gp" => AVCodecID_AV_CODEC_ID_H263, // 视你的来源定义
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

unsafe fn map_audio_codec_id(s: &str) -> AVCodecID {
    match s {
        "g711" | "pcma" => AVCodecID_AV_CODEC_ID_PCM_ALAW,   // 如果确定是 A-law
        "pcmu" => AVCodecID_AV_CODEC_ID_PCM_MULAW,
        "g723" => AVCodecID_AV_CODEC_ID_G723_1,
        "g729" => AVCodecID_AV_CODEC_ID_G729,
        "g722" => AVCodecID_AV_CODEC_ID_ADPCM_G722,
        "aac"  => AVCodecID_AV_CODEC_ID_AAC,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

// --- 辅助：根据 MediaExt 补齐/覆盖一个 AVStream 的参数 ---
unsafe fn fill_stream_from_media_ext(stream: *mut AVStream, media_ext: &MediaExt, prefer_missing_only: bool) {
    if stream.is_null() { return; }
    let par = (*stream).codecpar;
    if par.is_null() { return; }

    let is_video_hint = media_ext.video_params.codec_id.is_some() || media_ext.video_params.resolution.is_some();
    let is_audio_hint = media_ext.audio_params.codec_id.is_some();

    // 1) 媒体类型
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_UNKNOWN {
        (*par).codec_type = if is_video_hint { AVMediaType_AVMEDIA_TYPE_VIDEO } else { AVMediaType_AVMEDIA_TYPE_AUDIO };
    }

    // 2) codec_id
    if is_video_hint {
        if let Some(ref s) = media_ext.video_params.codec_id {
            let id = map_video_codec_id(s);
            if id != AVCodecID_AV_CODEC_ID_NONE {
                if !prefer_missing_only || (*par).codec_id == AVCodecID_AV_CODEC_ID_NONE {
                    (*par).codec_id = id;
                }
            }
        }
    } else if is_audio_hint {
        if let Some(ref s) = media_ext.audio_params.codec_id {
            let id = map_audio_codec_id(s);
            if id != AVCodecID_AV_CODEC_ID_NONE {
                if !prefer_missing_only || (*par).codec_id == AVCodecID_AV_CODEC_ID_NONE {
                    (*par).codec_id = id;
                }
            }
        }
    }

    // 3) 分辨率（仅视频）
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
        if let Some((w, h)) = media_ext.video_params.resolution {
            if !prefer_missing_only || ((*par).width == 0 && (*par).height == 0) {
                (*par).width = w;
                (*par).height = h;
            }
        }
    }

    // 4) 比特率（视频/音频）
    if let Some(br_kbps) = media_ext.video_params.bitrate {
        let br = (br_kbps as i64) * 1000;
        if !prefer_missing_only || (*par).bit_rate <= 0 {
            (*par).bit_rate = br;
        }
    } else if let Some(ref br_str) = media_ext.audio_params.bitrate {
        if let Ok(br_kbps) = br_str.parse::<i64>() {
            if !prefer_missing_only || (*par).bit_rate <= 0 {
                (*par).bit_rate = br_kbps * 1000;
            }
        }
    }

    // 5) 采样率（仅音频）
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
        if let Some(ref sr_str) = media_ext.audio_params.sample_rate {
            if let Ok(mut sr) = sr_str.parse::<i32>() {
                // 智能判断：< 1000 认为是 kHz，>= 1000 认为是 Hz
                if sr > 0 && sr < 1000 { sr *= 1000; }
                if !prefer_missing_only || (*par).sample_rate <= 0 {
                    (*par).sample_rate = sr;
                }
            }
        }
    }

    // 6) 帧率/时间基（视频）
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
        if let Some(fps) = media_ext.video_params.fps {
            if !prefer_missing_only || ((*stream).avg_frame_rate.num == 0 || (*stream).avg_frame_rate.den == 0) {
                (*stream).avg_frame_rate = AVRational { num: fps, den: 1 };
            }
            if !prefer_missing_only || ((*stream).r_frame_rate.num == 0 || (*stream).r_frame_rate.den == 0) {
                (*stream).r_frame_rate = AVRational { num: fps, den: 1 };
            }
            if !prefer_missing_only || ((*stream).time_base.num == 0 || (*stream).time_base.den == 0) {
                // 若是 PS/TS（clock_rate=90000）则优先 1/90000；否则按 fps
                if media_ext.clock_rate > 0 {
                    (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
                } else {
                    (*stream).time_base = AVRational { num: 1, den: fps.max(1) };
                }
            }
        } else {
            // 没提供 fps，也把 time_base 设置为 clock_rate（若有效）
            if media_ext.clock_rate > 0 && (!prefer_missing_only || ((*stream).time_base.num == 0 || (*stream).time_base.den == 0)) {
                (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
            }
        }
    }
}

impl DemuxerContext {
    pub fn start_demuxer(_ssrc: u32, media_ext: &MediaExt, rtp_buffer: rtp::RtpPacketBuffer) -> GlobalResult<Self> {
        unsafe {
            // 1) alloc fmt_ctx
            let mut fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;

            // 2) 设置 AVDictionary（注意：不要用 vcodec/video_size 这种仅 raw 有效的参数）
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();

            // fflags
            let fflags = CString::new("fflags").unwrap();
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts+ignidx").unwrap();
            av_dict_set(
                &mut dict_opts,
                fflags.as_ptr(),
                fflags_val.as_ptr(),
                0,
            );

            // strict
            let strict = CString::new("strict").unwrap();
            let strict_val = CString::new("experimental").unwrap();
            av_dict_set(
                &mut dict_opts,
                strict.as_ptr(),
                strict_val.as_ptr(),
                0,
            );

            // analyzeduration / probesize：给 demuxer 提示，但不再指望靠它填参数
            let ans = CString::new("analyzeduration").unwrap();
            let ans_val = CString::new("1000000").unwrap();// 1s
            av_dict_set(
                &mut dict_opts,
                ans.as_ptr(),
                ans_val.as_ptr(), 
                0,
            );
            // let probesize = CString::new("probesize").unwrap();
            // let probesize_val = CString::new("32768").unwrap(); // 32 KiB
            // av_dict_set(
            //     &mut dict_opts,
            //     probesize.as_ptr(),
            //     probesize_val.as_ptr(), 
            //     0,
            // );

            // 3) input fmt 选择
            let format_name = match media_ext.type_name.as_str() {
                "PS" => "mpeg",
                "H264" => "h264", // raw H264 时才有效
                "H265" => "hevc",
                "AAC" => "aac",
                "G711" => "pcm_alaw",
                _ => {
                    if let Some(codec_id) = &media_ext.video_params.codec_id {
                        match codec_id.as_str() {
                            "h264" => "h264",
                            "h265" => "hevc",
                            _ => "mpeg",
                        }
                    } else {
                        "mpeg"
                    }
                }
            };

            info!("Using input format: {}", format_name);
            let ps = CString::new(format_name).unwrap();
            let mut input_fmt = av_find_input_format(ps.as_ptr());
            if input_fmt.is_null() {
                warn!("{} demuxer not found, trying default mpeg", format_name);
                let mpeg_fmt = CString::new("mpeg").unwrap();
                input_fmt = av_find_input_format(mpeg_fmt.as_ptr());
                if input_fmt.is_null() {
                    input_fmt = av_find_input_format(ptr::null());
                }
            }

            // 4) custom AVIO
            let io_ctx_buffer = av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
            if io_ctx_buffer.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to allocate IO buffer", |msg| error!("{msg}")));
            }

            let rtp_buf_ptr = Box::into_raw(Box::new(rtp_buffer)) as *mut std::ffi::c_void;

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
                av_free(io_ctx_buffer as *mut std::ffi::c_void);
                avformat_free_context(fmt_ctx);
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| error!("{msg}")));
            }
            (*io_ctx).seekable = 0;
            (*fmt_ctx).pb = io_ctx;

            // 5) open input
            let input_url = ptr::null();
            let ret_open = avformat_open_input(&mut fmt_ctx, input_url, input_fmt, &mut dict_opts);
            if ret_open < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret_open);
                avio_context_free(&mut io_ctx);
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_buf_ptr as *mut _));
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open input: {}", msg)));
            }

            // 6) find stream info（正常 probe）
            let mut retry_count = 0;
            let max_retries = 3;
            let mut ret_fsi = avformat_find_stream_info(fmt_ctx, &mut dict_opts);

            while ret_fsi < 0 && retry_count < max_retries {
                warn!("avformat_find_stream_info failed (attempt {}), retrying...", retry_count + 1);
                std::thread::sleep(std::time::Duration::from_millis(500));
                ret_fsi = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
                retry_count += 1;
            }

            if ret_fsi < 0 {
                warn!("Failed to find stream info after {} attempts, will fallback to MediaExt.", max_retries);
            }

            let nb_streams = (*fmt_ctx).nb_streams as usize;

            // 7) （关键）对每个 AVStream：若探测失败或缺字段，用 MediaExt 补齐/覆盖
            for i in 0..nb_streams {
                let st = *(*fmt_ctx).streams.add(i);
                if st.is_null() { continue; }

                if ret_fsi < 0 {
                    // 探测失败：强制覆盖
                    fill_stream_from_media_ext(st, media_ext, /*prefer_missing_only*/ false);
                } else {
                    // 探测成功：仅补齐缺失字段，不盲目覆盖
                    fill_stream_from_media_ext(st, media_ext, /*prefer_missing_only*/ true);
                }

                // 额外健壮性：若仍无 codec_id，且知道是视频（PS/90000 + video_params），再兜底一次
                if (*(*st).codecpar).codec_id == AVCodecID_AV_CODEC_ID_NONE {
                    if media_ext.video_params.codec_id.is_some() {
                        fill_stream_from_media_ext(st, media_ext, false);
                    }
                }
            }

            // 8) 拷贝出 codecpar_list / stream_mapping
            let nb_streams = (*fmt_ctx).nb_streams as usize;
            let mut codecpar_list = Vec::with_capacity(nb_streams);
            let mut stream_mapping = Vec::with_capacity(nb_streams);

            for i in 0..nb_streams {
                let st = *(*fmt_ctx).streams.add(i);
                let codecpar = avcodec_parameters_alloc();
                if codecpar.is_null() {
                    avformat_close_input(&mut fmt_ctx);
                    return Err(GlobalError::new_sys_error("Failed to allocate codec parameters", |msg| error!("{msg}")));
                }
                avcodec_parameters_copy(codecpar, (*st).codecpar);

                let is_video = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;
                info!(
                    "Stream {}: type={}, codec_id={}, w={}, h={}, br={}, tb={}/{} fps={} (avg)",
                    i,
                    (*codecpar).codec_type,
                    (*codecpar).codec_id,
                    (*codecpar).width,
                    (*codecpar).height,
                    (*codecpar).bit_rate,
                    (*st).time_base.num,
                    (*st).time_base.den,
                    if (*st).avg_frame_rate.den != 0 {
                        (*st).avg_frame_rate.num / (*st).avg_frame_rate.den
                    } else { 0 }
                );

                codecpar_list.push(codecpar);
                stream_mapping.push((i, is_video));
            }

            rsmpeg::ffi::av_dict_free(&mut dict_opts);

            Ok(Self {
                avio: Arc::new(AvioResource {
                    fmt_ctx,
                    io_buf: io_ctx_buffer, // 释放由 avio_context_free 负责
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
