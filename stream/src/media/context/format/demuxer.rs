use crate::media::{rtp, rw, show_ffmpeg_error_msg, DEFAULT_IO_BUF_SIZE};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use rsmpeg::ffi::{av_find_input_format, av_free, avcodec_find_decoder, avcodec_parameters_alloc, avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context, avio_context_free, AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_ADPCM_G722, AVCodecID_AV_CODEC_ID_G723_1, AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H263, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_MPEG4, AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW, AVCodecID_AV_CODEC_ID_SIREN, AVCodecParameters, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO, AVRational, AVStream};
use shared::info::media_info_ext::MediaExt;
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::Arc;
use crate::media::context::RtpState;


/// 不会 free RtpState（因为它只是一个裸指针）。
/// RtpState 的内存必须由 MediaContext 或上层独占释放。
type OpaquePtr = *mut (rtp::RtpPacketBuffer, *mut RtpState);

/// Wrapper that owns FFmpeg resources for an input (fmt_ctx + avio_ctx + io_buf + opaque)
pub struct AvioResource {
    pub fmt_ctx: *mut AVFormatContext,
    /// raw buffer pointer passed to avio_alloc_context
    pub io_buf: *mut u8,
    pub avio_ctx: *mut AVIOContext,
}
unsafe impl Send for AvioResource {} // only safe if you ensure no concurrent mutable use across threads

impl Drop for AvioResource {
    fn drop(&mut self) {
        unsafe {
            // 1) 读取并释放opaque（优先处理）	
            let mut opaque_ptr = ptr::null_mut();
            if !self.avio_ctx.is_null() {
                opaque_ptr = (*self.avio_ctx).opaque;
                (*self.avio_ctx).opaque = ptr::null_mut(); // 解除关联	
            }
            if !opaque_ptr.is_null() {
                let tup_ptr = opaque_ptr as OpaquePtr;
                drop(Box::from_raw(tup_ptr)); // 安全回收	
            }
            // 2) 释放avio_ctx（内部会释放io_buf）	
            if !self.avio_ctx.is_null() {
                let mut local = self.avio_ctx;
                avio_context_free(&mut local); // 自动释放io_buf	
                self.avio_ctx = ptr::null_mut();
            }
            // 3) 关闭fmt_ctx	
            if !self.fmt_ctx.is_null() {
                (*self.fmt_ctx).pb = ptr::null_mut();
                let mut local_fmt = self.fmt_ctx;
                avformat_close_input(&mut local_fmt);
                self.fmt_ctx = ptr::null_mut();
            }
            // 4) 移除手动释放io_buf的代码	
            self.io_buf = ptr::null_mut(); // 无需手动释放	
        }
    }
}

#[derive(Clone)]
pub struct DemuxerContext {
    pub avio: Arc<AvioResource>,
    /// we own `*mut AVCodecParameters` pointers and must free them in Drop
    pub codecpar_list: Vec<*mut rsmpeg::ffi::AVCodecParameters>,
    /// (input_stream_index, is_video)
    pub stream_mapping: Vec<(usize, bool)>,
}
impl Drop for DemuxerContext {
    fn drop(&mut self) {
        unsafe {
            for &par in &self.codecpar_list {
                if !par.is_null() {
                    // avcodec_parameters_free takes *mut *mut AVCodecParameters
                    let mut p = par;
                    avcodec_parameters_free(&mut p);
                    // p now NULL
                }
            }
        }
    }
}

/// Helper: create an AVFormatContext and set custom IO flag
unsafe fn alloc_fmt_ctx_with_custom_io() -> GlobalResult<*mut AVFormatContext> { unsafe {
    let fmt_ctx = avformat_alloc_context();
    if fmt_ctx.is_null() {
        return Err(GlobalError::new_sys_error(
            "Failed to alloc format context",
            |msg| error!("{msg}"),
        ));
    }
    // mark we will use custom IO
    (*fmt_ctx).flags |= rsmpeg::ffi::AVFMT_FLAG_CUSTOM_IO as c_int;
    Ok(fmt_ctx)
}}

/// Helper: allocate AVIOContext with boxed opaque tuple; returns (pb, boxed_ptr, buf_ptr)
unsafe fn alloc_avio_for_rtp(
    rtp_buffer: rtp::RtpPacketBuffer,
    rtp_state: *mut RtpState,
) -> GlobalResult<(*mut AVIOContext, *mut c_void, *mut u8)> { unsafe {
    // allocate IO buffer
    let io_buf = rsmpeg::ffi::av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
    if io_buf.is_null() {
        return Err(GlobalError::new_sys_error(
            "Failed to allocate IO buffer",
            |msg| error!("{msg}"),
        ));
    }

    // Box the tuple and leak into raw pointer; opaque owns the rtp buffer and ptr to rtp_state
    let boxed = Box::new((rtp_buffer, rtp_state));
    let opaque = Box::into_raw(boxed) as *mut c_void;

    // create avio ctx
    let pb = avio_alloc_context(
        io_buf,
        DEFAULT_IO_BUF_SIZE as c_int,
        0,
        opaque,
        Some(rw::read_rtp_payload), // your read callback
        None,
        None,
    );
    if pb.is_null() {
        // cleanup: free io_buf and boxed opaque
        // restore Box to drop it
        let tup = opaque as OpaquePtr;
        drop(Box::from_raw(tup));
        av_free(io_buf as *mut c_void);
        return Err(GlobalError::new_sys_error(
            "Failed to allocate AVIO context",
            |msg| error!("{msg}"),
        ));
    }

    // ensure pb isn't marked seekable
    (*pb).seekable = 0;
    Ok((pb, opaque, io_buf))
}}

/// Helper: open input with given format and dict opts (dict ownership: caller frees)
unsafe fn open_input_with_format(
    fmt_ctx: *mut AVFormatContext,
    input_fmt: *mut rsmpeg::ffi::AVInputFormat,
    dict_opts: *mut AVDictionary,
) -> Result<(), GlobalError> { unsafe {
    let ret = avformat_open_input(&mut (fmt_ctx as *mut _), ptr::null(), input_fmt, &mut (dict_opts as *mut _));
    if ret < 0 {
        let ffmpeg_error = show_ffmpeg_error_msg(ret);
        return Err(GlobalError::new_biz_error(1100, &ffmpeg_error, |msg| error!("{msg}")));
    }
    Ok(())
}}

/// Helper: find stream info with retries
unsafe fn find_stream_info_with_retry(
    fmt_ctx: *mut AVFormatContext,
    dict_opts: *mut AVDictionary,
    media_ext: &MediaExt,
) -> Result<(), GlobalError> { unsafe {
    let mut retry_count = 0usize;
    let max_retries = 3usize;
    let mut ret = avformat_find_stream_info(fmt_ctx, &mut (dict_opts as *mut _));
    let mut not_the_codec = false;
    if ret >= 0 {
        not_the_codec = !check_codec(fmt_ctx, media_ext);
    }
    while (ret < 0 || not_the_codec) && retry_count < max_retries {
        info!("avformat_find_stream_info failed (attempt {}), retrying...", retry_count + 1);
        std::thread::sleep(std::time::Duration::from_millis(500));
        ret = avformat_find_stream_info(fmt_ctx, &mut (dict_opts as *mut _));
        if ret >= 0 {
            not_the_codec = !check_codec(fmt_ctx, media_ext);
        }
        retry_count += 1;
    }
    if ret < 0 {
        // Not fatal in all cases, but here we treat as error (you can relax to Ok)
        return Err(GlobalError::new_sys_error(
            "Failed to find stream info after max_retries attempts",
            |msg| error!("{msg}"),
        ));
    }
    Ok(())
}}

unsafe fn check_codec(fmt_ctx: *mut AVFormatContext, media_ext: &MediaExt) -> bool { unsafe {
    let nb_streams = (*fmt_ctx).nb_streams as usize;
    for i in 0..nb_streams {
        let st = *(*fmt_ctx).streams.add(i);
        if st.is_null() {
            return false;
        }
        // 判断当前流是视频还是音频，并与 media_ext 中的参数进行比对，不一致则返回错误
        let codecpar = (*st).codecpar;
        let is_video_stream = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;
        let is_audio_stream = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO;
        if is_video_stream {
            if let Some(ref v_codec) = media_ext.video_params.codec_id {
                let expected_id = map_video_codec_id(v_codec);
                if expected_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != expected_id {
                    warn!("视频流 codec_id 不一致: demuxer={}, media_ext={}", (*codecpar).codec_id, expected_id);
                    return false;
                }
            }
        } else if is_audio_stream {
            if let Some(ref a_codec) = media_ext.audio_params.codec_id {
                let expected_id = map_audio_codec_id(a_codec);
                if expected_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != AVCodecID_AV_CODEC_ID_NONE && (*codecpar).codec_id != expected_id {
                    warn!("音频流 codec_id 不一致: demuxer={}, media_ext={}", (*codecpar).codec_id, expected_id);
                    return false;
                }
            }
        }
    }
    true
}}

/// Helper: cleanup when start_demuxer fails before AvioResource is returned.
/// This will free io_ctx (if non-null), io_buf (if non-null) and boxed opaque (if non-null).
unsafe fn cleanup_early(
    io_ctx: *mut AVIOContext,
    opaque_ptr: *mut c_void,
    io_buf: *mut u8,
) { unsafe {
    // If avio ctx exists, clear its opaque and free it
    if !io_ctx.is_null() {
        // detach opaque to avoid double-free inside avio_context_free
        (*io_ctx).opaque = ptr::null_mut();
        let mut local_io = io_ctx;
        avio_context_free(&mut local_io); // sets to NULL
    }
    // free buffer if allocated
    if !io_buf.is_null() {
        av_free(io_buf as *mut c_void);
    }
    // drop boxed opaque if allocated
    if !opaque_ptr.is_null() {
        let tup = opaque_ptr as OpaquePtr;
        // Safety: only call when we are sure AvioResource was not created to own it.
        drop(Box::from_raw(tup));
    }
}}

/// Map helper functions (you provided earlier, included here for completeness)
unsafe fn map_video_codec_id(s: &str) -> AVCodecID {
    match s.to_lowercase().as_str() {
        "h264" => AVCodecID_AV_CODEC_ID_H264,
        "h265" | "hevc" => AVCodecID_AV_CODEC_ID_HEVC,
        "mpeg4" => AVCodecID_AV_CODEC_ID_MPEG4,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        "3gp" => AVCodecID_AV_CODEC_ID_H263, // 视来源定义
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

/// fill stream parameters from media_ext (your version, slightly simplified)
unsafe fn fill_stream_from_media_ext(stream: *mut AVStream, media_ext: &MediaExt) { unsafe {
    if stream.is_null() { return; }
    let par = (*stream).codecpar;
    if par.is_null() { return; }
    // Video
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
        if let Some((w, h)) = media_ext.video_params.resolution {
            if (*par).width <= 0 || (*par).height <= 0 {
                (*par).width = w;
                (*par).height = h;
            }
        }
        if let Some(br_kbps) = media_ext.video_params.bitrate {
            let br = (br_kbps as i64) * 1000;
            if (*par).bit_rate <= 0 {
                (*par).bit_rate = br;
            }
        }
        if (*stream).time_base.num <= 0 || (*stream).time_base.den <= 0 {
            if media_ext.clock_rate > 0 {
                (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
            }
        }
        if let Some(fps) = media_ext.video_params.fps {
            if (*stream).avg_frame_rate.num <= 0 || (*stream).avg_frame_rate.den <= 0 {
                (*stream).avg_frame_rate = AVRational { num: fps, den: 1 };
                (*stream).r_frame_rate = AVRational { num: fps, den: 1 };
            }
        } else if media_ext.clock_rate > 0 && ((*stream).time_base.num <= 0 || (*stream).time_base.den <= 0) {
            (*stream).time_base = AVRational { num: 1, den: media_ext.clock_rate };
        }
        debug!(
            "fill_stream: stream_id={:?} time_base={}/{} avg_frame_rate={}/{} r_frame_rate={}/{}",
            (*stream).id,
            (*stream).time_base.num, (*stream).time_base.den,
            (*stream).avg_frame_rate.num, (*stream).avg_frame_rate.den,
            (*stream).r_frame_rate.num, (*stream).r_frame_rate.den,
        );
    }

    // Audio
    if (*par).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
        if let Some(ref sr_str) = media_ext.audio_params.sample_rate {
            if let Ok(mut sr) = sr_str.parse::<i32>() {
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
    // 如果 extradata 缺失，调用 ensure_extradata
    //使用 av_bitstream_filter_init("h264_mp4toannexb") 创建过滤器上下文，然后在每次收到 AVPacket 后，用 av_bitstream_filter_filter 处理后再写入输出。
    if (*par).extradata.is_null() || is_extradata_incomplete((*par).codec_id, (*par).extradata_size) {
        let ret = ensure_extradata((*stream).codecpar, (*par).codec_id, stream);
        if ret == 0 {
            info!(
                "extradata filled: codec_id={} size={}",
                (*par).codec_id,
                (*par).extradata_size
            );
        } else {
            warn!(
                "extradata not filled: codec_id={}",
                (*par).codec_id
            );
        }
    } else {
        info!(
            "extradata already present: codec_id={} size={}",
            (*par).codec_id,
            (*par).extradata_size
        );
    }
    info!(
        "fill_stream: stream={:?} time_base={}/{} avg_frame_rate={}/{} r_frame_rate={}/{}",
        stream,
        (*stream).time_base.num, (*stream).time_base.den,
        (*stream).avg_frame_rate.num, (*stream).avg_frame_rate.den,
        (*stream).r_frame_rate.num, (*stream).r_frame_rate.den
    );
}}
fn is_extradata_incomplete(codec_id: AVCodecID, size: i32) -> bool {
    match codec_id {
        AVCodecID_AV_CODEC_ID_H264 => size < 50,
        AVCodecID_AV_CODEC_ID_HEVC => size < 80,
        AVCodecID_AV_CODEC_ID_AAC => size < 2,
        _ => false,
    }
}
unsafe fn ensure_extradata(
    codecpar: *mut AVCodecParameters,
    codec_id: AVCodecID,
    st: *mut AVStream,
) -> i32 { unsafe {
    use rsmpeg::ffi::*;

    match codec_id {
        // --- 视频 ---
        AVCodecID_AV_CODEC_ID_H264 => {
            return apply_bsf(st, "h264_mp4toannexb\0");
        }
        AVCodecID_AV_CODEC_ID_HEVC => {
            return apply_bsf(st, "hevc_mp4toannexb\0");
        }
        AVCodecID_AV_CODEC_ID_MPEG4 => {
            return apply_bsf(st, "mpeg4_unpack_bframes\0");
        }

        // --- 音频 ---
        AVCodecID_AV_CODEC_ID_AAC => {
            return apply_bsf(st, "aac_adtstoasc\0");
        }

        AVCodecID_AV_CODEC_ID_PCM_ALAW   // G.711 A-law
        | AVCodecID_AV_CODEC_ID_PCM_MULAW // G.711 μ-law
        | AVCodecID_AV_CODEC_ID_ADPCM_G722
        | AVCodecID_AV_CODEC_ID_G723_1
        | AVCodecID_AV_CODEC_ID_G729 => {
            // 这些 codec 不需要 extradata
            return 0;
        }

        _ => {
            warn!("No extradata handler for codec_id={}", codec_id);
            return -1;
        }
    }
}}

/// 使用 FFmpeg bsf 填充 extradata
unsafe fn apply_bsf(st: *mut AVStream, bsf_name: &str) -> i32 { unsafe {
    use rsmpeg::ffi::*;
    let mut bsf: *mut AVBSFContext = std::ptr::null_mut();
    let filter = av_bsf_get_by_name(bsf_name.as_ptr() as *const i8);
    if filter.is_null() {
        base::log::error!("BSF {} not found", bsf_name);
        return -1;
    }

    if av_bsf_alloc(filter, &mut bsf) < 0 {
        return -1;
    }

    avcodec_parameters_copy((*bsf).par_in, (*st).codecpar);
    if av_bsf_init(bsf) < 0 {
        av_bsf_free(&mut bsf);
        return -1;
    }

    // 应用一次 extradata 更新
    avcodec_parameters_copy((*st).codecpar, (*bsf).par_out);

    base::log::info!(
        "Applied bsf {} on stream, extradata size={}",
        bsf_name,
        (*(*st).codecpar).extradata_size
    );

    av_bsf_free(&mut bsf);
    0
}}


/// pick input format (reuse your logic)
fn pick_input_format(media_ext: &MediaExt) -> &'static str {
    match media_ext.type_name.as_str() {
        "PS" => "mpeg", // mpeg-ps
        "H264" => "h264",
        "H265" => "hevc",
        "AAC" => "aac",
        "G711" => "alaw",
        _ => "mpeg",
    }
}

/// The refactored start_demuxer broken into steps; returns DemuxerContext on success.
impl DemuxerContext {
    pub fn start_demuxer(
        _ssrc: u32,
        media_ext: &MediaExt,
        rtp_buffer: rtp::RtpPacketBuffer,
        rtp_state: *mut RtpState,
    ) -> GlobalResult<Self> {
        unsafe {
            // 0) pre-checks
            // allocate fmt_ctx
            let fmt_ctx = alloc_fmt_ctx_with_custom_io()?;

            // 1) pick input format
            let fmt_name = pick_input_format(media_ext);
            debug!("Using input format: {}", fmt_name);
            let ifmt_name = CString::new(fmt_name).unwrap();
            let input_fmt = av_find_input_format(ifmt_name.as_ptr());
            if input_fmt.is_null() {
                avformat_free_context(fmt_ctx);
                return Err(GlobalError::new_sys_error(&format!("demuxer not found: {}", fmt_name), |msg| error!("{msg}")));
            }

            // 2) alloc avio + boxed opaque
            let (pb, opaque_ptr, io_buf) = match alloc_avio_for_rtp(rtp_buffer, rtp_state) {
                Ok(t) => t,
                Err(e) => {
                    avformat_free_context(fmt_ctx);
                    return Err(e);
                }
            };

            // attach pb to fmt_ctx
            (*fmt_ctx).pb = pb;

            // 3) set codec hints if provided in media_ext
            if let Some(v_id) = &media_ext.video_params.codec_id {
                let id = map_video_codec_id(v_id);
                if id != AVCodecID_AV_CODEC_ID_NONE {
                    (*fmt_ctx).video_codec_id = id;
                    let codec = avcodec_find_decoder(id);
                    if codec.is_null() {
                        // cleanup: free resources we just allocated
                        cleanup_early(pb, opaque_ptr, io_buf);
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
                        cleanup_early(pb, opaque_ptr, io_buf);
                        avformat_free_context(fmt_ctx);
                        return Err(GlobalError::new_sys_error(&format!("Audio codec not found: {}", a_id), |msg| error!("{msg}")));
                    }
                    (*fmt_ctx).audio_codec = codec;
                }
            }

            // 4) build dictionary options
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();
            macro_rules! set_dict {
                ($k:expr, $v:expr) => {{
                    let key = CString::new($k).unwrap();
                    let val = CString::new($v).unwrap();
                    rsmpeg::ffi::av_dict_set(&mut dict_opts, key.as_ptr(), val.as_ptr(), 0);
                }};
            }
            set_dict!("fflags", "nobuffer+discardcorrupt+ignidx");
            set_dict!("analyzeduration", "1000000");
            set_dict!("probesize", "32768");
            set_dict!("fpsprobesize", "0");

            // 5) open input
            let open_ret = avformat_open_input(&mut (fmt_ctx as *mut _), ptr::null(), input_fmt, &mut dict_opts);
            if open_ret < 0 {
                // cleanup: free dict, avio, boxed opaque, io_buf, fmt_ctx
                rsmpeg::ffi::av_dict_free(&mut dict_opts);
                // avio_context_free and drop opaque handled by cleanup_early
                cleanup_early(pb, opaque_ptr, io_buf);
                avformat_free_context(fmt_ctx);
                let ffmpeg_error = show_ffmpeg_error_msg(open_ret);
                return Err(GlobalError::new_biz_error(1100, &ffmpeg_error, |msg| error!("{msg}")));
            }

            // 6) find stream info (with retry)
            if let Err(e) = find_stream_info_with_retry(fmt_ctx, dict_opts, media_ext) {
                // close input and cleanup early: after avformat_close_input, pb is detached, but
                avformat_close_input(&mut (fmt_ctx as *mut _));
                return Err(e);
            }

            // 8) collect codecpar_list & stream mapping
            let nb_streams = (*fmt_ctx).nb_streams as usize;
            let mut codecpar_list: Vec<*mut rsmpeg::ffi::AVCodecParameters> = Vec::with_capacity(nb_streams);
            let mut stream_mapping: Vec<(usize, bool)> = Vec::with_capacity(nb_streams);
            for i in 0..nb_streams {
                let st = *(*fmt_ctx).streams.add(i);
                fill_stream_from_media_ext(st, media_ext);
                let codecpar = avcodec_parameters_alloc();
                if codecpar.is_null() {
                    // cleanup: close input (which will free pb), and let AvioResource::drop handle opaque/io_buf
                    avformat_close_input(&mut (fmt_ctx as *mut _));
                    return Err(GlobalError::new_sys_error("Failed to allocate codec parameters", |msg| error!("{msg}")));
                }
                avcodec_parameters_copy(codecpar, (*st).codecpar);
                let is_video = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;
                codecpar_list.push(codecpar);
                stream_mapping.push((i, is_video));
                info!("Stream {}: codec_id={}, tb={}/{}", i, (*codecpar).codec_id, (*st).time_base.num, (*st).time_base.den);
            }

            // free dict
            rsmpeg::ffi::av_dict_free(&mut dict_opts);

            // 9) Build AvioResource and return DemuxerContext
            let avio_res = AvioResource {
                fmt_ctx,
                io_buf: io_buf,
                avio_ctx: pb,
            };

            Ok(DemuxerContext {
                avio: Arc::new(avio_res),
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
