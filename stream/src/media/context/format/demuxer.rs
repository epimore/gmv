use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{av_dict_set, av_find_input_format, av_free, av_malloc, avcodec_alloc_context3, avcodec_find_decoder, avcodec_parameters_alloc, avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context, avio_context_free, AVCodec, AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_ADPCM_G722, AVCodecID_AV_CODEC_ID_G723_1, AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H263, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_MPEG4, AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW, AVCodecID_AV_CODEC_ID_SIREN, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_UNKNOWN, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, AVStream, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_NOFILLIN};
use shared::info::media_info_ext::MediaExt;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;
use crate::media::context::RtpState;

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
            // 1) 先取出 opaque（如果有）
            let mut opaque_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            if !self.avio_ctx.is_null() {
                opaque_ptr = (*self.avio_ctx).opaque;
            }

            // 2) 释放 avio_ctx（会释放 AVIOContext 结构）
            if !self.avio_ctx.is_null() {
                // 把 opaque 从 avio_ctx 清除，避免 avio_context_free 对 opaque 做任何假设
                (*self.avio_ctx).opaque = std::ptr::null_mut();
                avio_context_free(&mut self.avio_ctx);
                // avio_context_free 不一定 free 你传入的 buffer -> 我们后面统一 free io_buf
            }

            // 3) 关闭 fmt_ctx（但先将 pb 置空，避免重复释放 pb）
            if !self.fmt_ctx.is_null() {
                if !(*self.fmt_ctx).pb.is_null() {
                    (*self.fmt_ctx).pb = std::ptr::null_mut();
                }
                avformat_close_input(&mut self.fmt_ctx);
                self.fmt_ctx = std::ptr::null_mut();
            }

            // 4) 释放 io_buf（如果有）—— 我们在 start_demuxer 保证 io_buf 仅在这里释放一次
            if !self.io_buf.is_null() {
                av_free(self.io_buf as *mut c_void);
                self.io_buf = ptr::null_mut();
            }

            // 5) 恢复并 drop opaque 的实际类型：(RtpPacketBuffer, *mut RtpState)
            if !opaque_ptr.is_null() {
                // 注意：opaque 是 Box::into_raw(Box::new((rtp_buffer, rtp_state)))
                let tup_ptr = opaque_ptr as *mut (rtp::RtpPacketBuffer, *mut RtpState);
                // 安全地把 Box 恢复并 drop（同时 drop rtp_buffer）
                drop(Box::from_raw(tup_ptr));
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
    match s.to_lowercase().as_str() {
        "h264" => AVCodecID_AV_CODEC_ID_H264,
        "h265" | "hevc" => AVCodecID_AV_CODEC_ID_HEVC,
        "mpeg4" => AVCodecID_AV_CODEC_ID_MPEG4,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        "3gp" => AVCodecID_AV_CODEC_ID_H263, // 视你的来源定义
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

pub unsafe fn map_audio_codec_id(s: &str) -> AVCodecID {
    match s.to_lowercase().as_str() {
        // G.711 A-law
        "g711" | "g711a" | "g.711a" | "g.711 a-law" | "a-law" | "alaw" | "pcma" | "pcm_alaw" =>
            AVCodecID_AV_CODEC_ID_PCM_ALAW,
        // G.711 μ-law
        "g711u" | "g.711u" | "g.711 μ-law" | "mu-law" | "mulaw" | "pcmu" | "pcm_mulaw" =>
            AVCodecID_AV_CODEC_ID_PCM_MULAW,
        // G.722
        "g722" | "g.722" =>
            AVCodecID_AV_CODEC_ID_ADPCM_G722,
        // G.722.1 (Siren)
        "g7221" | "g.722.1" | "siren" =>
            AVCodecID_AV_CODEC_ID_SIREN,
        // G.723.1
        "g723" | "g7231" | "g.723" | "g.723.1" | "g723_1" =>
            AVCodecID_AV_CODEC_ID_G723_1,
        // G.729
        "g729" | "g.729" =>
            AVCodecID_AV_CODEC_ID_G729,
        // AAC
        "aac" | "mpeg2-aac" | "mpeg4-aac" =>
            AVCodecID_AV_CODEC_ID_AAC,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

// --- 辅助：根据 MediaExt 补齐/覆盖一个 AVStream 的参数 ---
unsafe fn fill_stream_from_media_ext(stream: *mut AVStream, media_ext: &MediaExt) {
    if stream.is_null() { return; }
    let par = (*stream).codecpar;
    if par.is_null() { return; }

    // 视频
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
        //分辨率（仅视频）
        if let Some((w, h)) = media_ext.video_params.resolution {
            if (*par).width == 0 || (*par).height == 0 {
                (*par).width = w;
                (*par).height = h;
            }
        }
        // 比特率视频
        if let Some(br_kbps) = media_ext.video_params.bitrate {
            let br = (br_kbps as i64) * 1000;
            if (*par).bit_rate <= 0 {
                (*par).bit_rate = br;
            }
        }
        //帧率、时间基
        if let Some(fps) = media_ext.video_params.fps {
            if (*stream).avg_frame_rate.num == 0 || (*stream).avg_frame_rate.den == 0 {
                (*stream).avg_frame_rate = AVRational { num: fps, den: 1 };
            }
            if (*stream).r_frame_rate.num == 0 || (*stream).r_frame_rate.den == 0 {
                (*stream).r_frame_rate = AVRational { num: fps, den: 1 };
            }
            if (*stream).time_base.num == 0 || (*stream).time_base.den == 0 {
                // 若是 PS/TS（clock_rate=90000）则优先 1/90000；否则按 fps
                if media_ext.clock_rate > 0 {
                    (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
                } else {
                    (*stream).time_base = AVRational { num: 1, den: fps.max(1) };
                }
            }
        } else {
            // 没提供 fps，也把 time_base 设置为 clock_rate（若有效）
            if media_ext.clock_rate > 0 && ((*stream).time_base.num == 0 || (*stream).time_base.den == 0) {
                (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
            }
        }
    }

    // 音频
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
        if let Some(ref sr_str) = media_ext.audio_params.sample_rate {
            if let Ok(mut sr) = sr_str.parse::<i32>() {
                // 智能判断：< 1000 认为是 kHz，>= 1000 认为是 Hz
                if sr > 0 && sr < 1000 { sr *= 1000; }
                if (*par).sample_rate <= 0 {
                    (*par).sample_rate = sr;
                }
            }
        }
        if let Some(ref br_str) = media_ext.audio_params.bitrate {
            if let Ok(br_kbps) = br_str.parse::<i64>() {
                if (*par).bit_rate <= 0 {
                    (*par).bit_rate = br_kbps * 1000;
                }
            }
        }
    }

}

// --- 输入格式辅助：根据 media_ext 选择 demuxer 名称 ---
fn pick_input_format(media_ext: &MediaExt) -> &'static str {
    match media_ext.type_name.as_str() {
        "PS" => "mpeg",           // mpeg-ps demuxer
        "H264" => "h264",         // raw h264
        "H265" => "hevc",         // raw hevc
        "AAC" => "aac",          // raw aac (ADTS)
        "G711" => {
            // 尝试根据 codec_id 判别 A-Law / μ-Law（alaw / mulaw）
            if let Some(cid) = &media_ext.audio_params.codec_id {
                match cid.as_str() {
                    "g711" | "pcma" => "alaw",
                    "pcmu" => "mulaw",
                    _ => "alaw",
                }
            } else {
                "alaw"
            }
        }
        _ => {
            // 兜底：若 video codec 已知，且是原始裸流，优先对应 raw demuxer；否则走 mpeg-ps
            if let Some(cid) = &media_ext.video_params.codec_id {
                match cid.as_str() {
                    "h264" => "h264",
                    "h265" | "hevc" => "hevc",
                    _ => "mpeg",
                }
            } else {
                "mpeg"
            }
        }
    }
}

fn cstr(s: &str) -> Result<CString, GlobalError> {
    CString::new(s).map_err(|e| {
        GlobalError::new_biz_error(
            1001,
            &format!("Failed to create C string: {}", e),
            |msg| error!("{}", msg),
        )
    })
}

impl DemuxerContext {
    pub fn start_demuxer(_ssrc: u32, media_ext: &MediaExt, rtp_buffer: rtp::RtpPacketBuffer, rtp_state: *mut RtpState) -> GlobalResult<Self> {
        unsafe {
            // --- 1) 分配 fmt_ctx ---
            let mut fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;

            // --- 2) 输入格式 ---
            let format_name = pick_input_format(media_ext);
            debug!("Using input format: {}", format_name);
            let input_fmt = av_find_input_format(cstr(format_name)?.as_ptr());
            if input_fmt.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error(&format!("demuxer not found: {}", format_name), |msg| error!("{msg}")));
            }

            // --- 3) 分配 IO 缓冲区 ---
            let io_ctx_buffer = av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
            if io_ctx_buffer.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to allocate IO buffer", |msg| error!("{msg}")));
            }

            let rtp_ptr = Box::into_raw(Box::new((rtp_buffer, rtp_state))) as *mut c_void;

            let mut io_ctx = avio_alloc_context(
                io_ctx_buffer,
                DEFAULT_IO_BUF_SIZE as c_int,
                0,
                rtp_ptr,
                Some(rw::read_rtp_payload),
                None,
                None,
            );
            if io_ctx.is_null() {
                av_free(io_ctx_buffer as *mut c_void);
                avformat_free_context(fmt_ctx);
                let (rpb,_) = rtp_ptr as *mut (rtp::RtpPacketBuffer, *mut RtpState);
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_ptr as *mut _));
                return Err(GlobalError::new_sys_error("Failed to allocate AVIO context", |msg| error!("{msg}")));
            }
            (*io_ctx).seekable = 0;
            (*fmt_ctx).pb = io_ctx;

            // --- 4) 根据 MediaExt 设置 codec ---
            if let Some(v_id) = &media_ext.video_params.codec_id {
                let id = map_video_codec_id(v_id);
                if id != AVCodecID_AV_CODEC_ID_NONE {
                    (*fmt_ctx).video_codec_id = id;
                    let codec = avcodec_find_decoder(id);
                    if codec.is_null() {
                        avio_context_free(&mut io_ctx);
                        drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_ptr as *mut _));
                        avformat_free_context(fmt_ctx);
                        return Err(GlobalError::new_sys_error(&format!("Video codec not found: {}", v_id), |msg| error!("{msg}")));
                    }
                    (*fmt_ctx).video_codec = codec;
                }
            }
            if let Some(a_id) = &media_ext.audio_params.codec_id {
                let id = map_audio_codec_id(a_id);
                if id != AVCodecID_AV_CODEC_ID_NONE {
                    (*fmt_ctx).audio_codec_id = id;
                    let codec = avcodec_find_decoder(id);
                    if codec.is_null() {
                        avio_context_free(&mut io_ctx);
                        drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_ptr as *mut _));
                        avformat_free_context(fmt_ctx);
                        return Err(GlobalError::new_sys_error(&format!("Audio codec not found: {}", a_id), |msg| error!("{msg}")));
                    }
                    (*fmt_ctx).audio_codec = codec;
                }
            }

            // --- 5) 设置 AVDictionary ---
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();
            macro_rules! set_dict {
                ($key:expr, $val:expr) => {{
                    let key = cstr($key)?;
                    let val = cstr($val)?;
                    av_dict_set(&mut dict_opts, key.as_ptr(), val.as_ptr(), 0);
                }};
            }
            set_dict!("fflags", "nobuffer+discardcorrupt+ignidx"); //genpts 去掉 
            set_dict!("analyzeduration", "1000000");
            set_dict!("probesize", "32768");
            set_dict!("fpsprobesize", "0");

            // --- 6) 打开输入 ---
            let ret_open = avformat_open_input(&mut fmt_ctx, ptr::null(), input_fmt, &mut dict_opts);
            if ret_open < 0 {
                rsmpeg::ffi::av_dict_free(&mut dict_opts);
                let ffmpeg_error = show_ffmpeg_error_msg(ret_open);
                avio_context_free(&mut io_ctx);
                drop(Box::<rtp::RtpPacketBuffer>::from_raw(rtp_ptr as *mut _));
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_biz_error(1100, &ffmpeg_error, |msg| error!("Failed to open input: {}", msg)));
            }

            // --- 7) 探测 stream info ---
            let mut retry_count = 0;
            let max_retries = 3;
            let mut ret_fsi = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
            while ret_fsi < 0 && retry_count < max_retries {
                info!("avformat_find_stream_info failed (attempt {}), retrying...", retry_count + 1);
                std::thread::sleep(std::time::Duration::from_millis(500));
                ret_fsi = avformat_find_stream_info(fmt_ctx, &mut dict_opts);
                retry_count += 1;
            }
            if ret_fsi < 0 {
                avformat_close_input(&mut fmt_ctx);
                return Err(GlobalError::new_sys_error("Failed to find stream info after max_retries attempts", |msg| error!("{msg}")));
            }

            // --- 8) 用 MediaExt 补齐 stream 参数 ---
            let nb_streams = (*fmt_ctx).nb_streams as usize;
            for i in 0..nb_streams {
                let st = *(*fmt_ctx).streams.add(i);
                if st.is_null() { continue; }
                // 判断当前流是视频还是音频，并与 media_ext 中的参数进行比对，不一致则返回错误
                let codecpar = (*st).codecpar;
                let is_video_stream = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;
                let is_audio_stream = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO;

                if is_video_stream {
                    if let Some(ref v_codec) = media_ext.video_params.codec_id {
                        let expected_id = map_video_codec_id(v_codec);
                        if expected_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != expected_id {
                            avformat_close_input(&mut fmt_ctx);
                            return Err(GlobalError::new_sys_error(
                                &format!("视频流 codec_id 不一致: demuxer={}, media_ext={}", (*codecpar).codec_id, expected_id),
                                |msg| error!("{msg}")
                            ));
                        }
                    }
                } else if is_audio_stream {
                    if let Some(ref a_codec) = media_ext.audio_params.codec_id {
                        let expected_id = map_audio_codec_id(a_codec);
                        if expected_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != expected_id {
                            avformat_close_input(&mut fmt_ctx);
                            return Err(GlobalError::new_sys_error(
                                &format!("音频流 codec_id 不一致: demuxer={}, media_ext={}", (*codecpar).codec_id, expected_id),
                                |msg| error!("{msg}")
                            ));
                        }
                    }
                }

                fill_stream_from_media_ext(st, media_ext);
            }

            // --- 9) 拷贝 codecpar_list / stream_mapping ---
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
                    io_buf: io_ctx_buffer,
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
    use rsmpeg::ffi::{av_demuxer_iterate, av_opt_next, AVOption};
    use std::ffi::CStr;
    use std::ptr;

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
