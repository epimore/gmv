use crate::media::context::format::demuxer::{
    DemuxerContext, H264ParameterSets, H265ParameterSets, ParamRepairState,
};
use log::{debug, error, info, warn};
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVCodec, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264,
    AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_OPUS, AVCodecID_AV_CODEC_ID_PCM_ALAW,
    AVCodecID_AV_CODEC_ID_PCM_MULAW, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVPixelFormat, AVPixelFormat_AV_PIX_FMT_NV12,
    AVPixelFormat_AV_PIX_FMT_YUV420P, AVPixelFormat_AV_PIX_FMT_YUVJ420P, AVRational,
    AVSampleFormat, AVSampleFormat_AV_SAMPLE_FMT_FLTP, AVSampleFormat_AV_SAMPLE_FMT_S16, AVStream,
    av_free, av_malloc, av_rescale_q, avcodec_alloc_context3, avcodec_close, avcodec_find_decoder,
    avcodec_free_context, avcodec_open2, avcodec_parameters_from_context,
    avcodec_parameters_to_context,
};
use shared::info::media_info_ext::MediaExt;
use std::ptr;

/// codec_type /codec_id /nb_streams :由demuxer->find_stream_info校验,进入不了此处
//修复媒体流基本信息:h264/h265/aac/g711
//video: timebase /width,height /format /extradata
//audio: sample_rate /channels /channel_layout /asc
//time: time_base /avg_frame_rate /pcr_value

pub unsafe fn repair_basic_stream_info(
    stream: *mut AVStream,
    pkt: &AVPacket,
    media_ext: &MediaExt,
    param: &mut ParamRepairState,
) -> bool {
    let mut all_ready = true;
    let par = (*stream).codecpar;
    let codec_type = (*par).codec_type;
    let codec_id = (*par).codec_id;

    debug!(
        "repair_basic_stream_info: stream_id={}, codec_type={}, codec_id={}",
        (*stream).id,
        codec_type,
        codec_id
    );

    // === 1. 修复时间基 (time_base) ===
    if (*stream).time_base.num <= 0 || (*stream).time_base.den <= 0 {
        match codec_type {
            AVMediaType_AVMEDIA_TYPE_VIDEO => {
                if (*stream).time_base.num <= 0 || (*stream).time_base.den <= 0 {
                    // 视频时间基H264/H265:使用默认值90000
                    let mut default = true;
                    if media_ext.clock_rate > 0 {
                        (*stream).time_base = AVRational {
                            num: 1,
                            den: media_ext.clock_rate,
                        };
                        debug!(
                            "Set video time_base from media_ext: 1/{}",
                            media_ext.clock_rate
                        );
                        default = false;
                    }
                    if default {
                        (*stream).time_base = AVRational { num: 1, den: 90000 }; // 90kHz
                        debug!("Set default video time_base: 1/90000");
                    }
                }
            }
            AVMediaType_AVMEDIA_TYPE_AUDIO => {
                // 音频采样率AAC/G711: 通常使用采样率8000
                let mut default = true;
                if (*stream).time_base.num <= 0 || (*stream).time_base.den <= 0 {
                    if let Some(ref sr_str) = media_ext.audio_params.sample_rate {
                        if let Ok(mut sr) = sr_str.parse::<i32>() {
                            if sr > 0 && sr < 1000 {
                                sr *= 1000;
                            } // 处理kHz单位
                            (*stream).time_base = AVRational { num: 1, den: sr };
                            debug!("Set audio time_base from sample_rate: 1/{}", sr);
                            default = false;
                        }
                    }
                    if default {
                        (*stream).time_base = AVRational { num: 1, den: 48000 }; // 48kHz
                        debug!("Set default audio time_base: 1/48000");
                    }
                }
            }
            _ => {
                // 其他类型流使用默认时间基
                (*stream).time_base = AVRational { num: 1, den: 90000 };
            }
        }
    }

    // === 2. 根据流类型进行特定修复 ===
    match codec_type {
        AVMediaType_AVMEDIA_TYPE_VIDEO => {
            // 先判断填充extradata
            match (*par).codec_id {
                AVCodecID_AV_CODEC_ID_H264 => {
                    if (*par).extradata_size < 15 || (*par).extradata.is_null() {
                        all_ready = false;
                        if repair_codecpar(stream, pkt, param) {
                            rebuild_par_from_extradata(stream);
                            all_ready = true;
                            // repair_video_stream_info(stream, param, media_ext);
                        }
                    }
                }
                AVCodecID_AV_CODEC_ID_HEVC => {
                    if (*par).extradata_size < 23 || (*par).extradata.is_null() {
                        all_ready = false;
                        if repair_codecpar(stream, pkt, param) {
                            rebuild_par_from_extradata(stream);
                            all_ready = true;
                            // repair_video_stream_info(stream, param, media_ext);
                        }
                    }
                }
                OTHER => {
                    warn!("unsupported codec_id = {}", OTHER)
                }
            }
        }
        AVMediaType_AVMEDIA_TYPE_AUDIO => {
            if matches!((*par).codec_id, AVCodecID_AV_CODEC_ID_AAC)
                && ((*par).extradata_size < 2 || (*par).extradata.is_null())
            {
                all_ready = false;
                if repair_codecpar(stream, pkt, param) {
                    all_ready = true;
                    repair_audio_stream_info(stream, media_ext);
                }
            } else {
                repair_audio_stream_info(stream, media_ext);
            }
        }
        _ => {
            // 其他类型流，只修复基本信息
            debug!("Skipping repair for non-A/V stream type: {}", codec_type);
        }
    }

    // === 3. 修复帧率信息：默认25 ===
    if codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
        if (*stream).avg_frame_rate.num <= 0 || (*stream).avg_frame_rate.den <= 0 {
            let mut default = true;
            if let Some(fps) = media_ext.video_params.fps {
                (*stream).avg_frame_rate = AVRational { num: fps, den: 1 };
                (*stream).r_frame_rate = AVRational { num: fps, den: 1 };
                debug!("Set frame_rate from media_ext: {}/1", fps);
                default = false;
            }
            if default {
                (*stream).avg_frame_rate = AVRational { num: 25, den: 1 };
                (*stream).r_frame_rate = AVRational { num: 25, den: 1 };
            }
        }
    }

    // === 4. 打印修复后的状态 ===
    debug!(
        "repair_basic_stream_info result for stream {}: time_base={}/{}, all_ready={}",
        (*stream).id,
        (*stream).time_base.num,
        (*stream).time_base.den,
        all_ready
    );

    all_ready
}

//将已成功修复的extradata再次解析到codecpar
unsafe fn rebuild_par_from_extradata(stream: *mut AVStream) -> bool {
    let par = (*stream).codecpar;
    let codec = avcodec_find_decoder((*par).codec_id);
    if codec.is_null() {
        return false;
    }
    let codec_ctx = avcodec_alloc_context3(codec);
    if codec_ctx.is_null() {
        return false;
    }
    // 将 par 中的 extradata "应用" 到 codec_ctx;触发 FFmpeg 内部的 SPS/PPS 解析逻辑
    let ret = avcodec_parameters_to_context(codec_ctx, par);
    if ret < 0 {
        avcodec_free_context(&mut (codec_ctx as *mut _));
        return false;
    }
    // 打开 codec 确保参数被完全解析
    let ret = avcodec_open2(codec_ctx, codec, ptr::null_mut());
    if ret < 0 {
        avcodec_free_context(&mut (codec_ctx as *mut _));
        return false;
    }
    //从解码器上下文获取更新后的参数
    let ret = avcodec_parameters_from_context(par, codec_ctx);
    if ret < 0 {
        avcodec_free_context(&mut (codec_ctx as *mut _));
        return false;
    }
    avcodec_free_context(&mut (codec_ctx as *mut _));
    true
}

fn for_each_nalu_annexb(data: &[u8], mut f: impl FnMut(&[u8])) {
    let mut i = 0;
    while i + 4 <= data.len() {
        let start = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            i + 3
        } else {
            i += 1;
            continue;
        };

        let mut end = start;
        while end + 3 < data.len()
            && !data[end..].starts_with(&[0, 0, 0, 1])
            && !data[end..].starts_with(&[0, 0, 1])
        {
            end += 1;
        }

        f(&data[start..end]);
        i = end;
    }
}
fn extract_h264_ps(pkt: &AVPacket, ps: &mut H264ParameterSets) {
    unsafe {
        let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);

        for_each_nalu_annexb(data, |nalu| {
            let nal_type = nalu[0] & 0x1F;
            match nal_type {
                7 if ps.sps.is_none() => ps.sps = Some(nalu.to_vec()),
                8 if ps.pps.is_none() => ps.pps = Some(nalu.to_vec()),
                _ => {}
            }
        });
    }
}
fn extract_h265_ps(pkt: &AVPacket, ps: &mut H265ParameterSets) {
    unsafe {
        let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);

        for_each_nalu_annexb(data, |nalu| {
            let nal_type = (nalu[0] >> 1) & 0x3F;
            match nal_type {
                32 if ps.vps.is_none() => ps.vps = Some(nalu.to_vec()),
                33 if ps.sps.is_none() => ps.sps = Some(nalu.to_vec()),
                34 if ps.pps.is_none() => ps.pps = Some(nalu.to_vec()),
                _ => {}
            }
        });
    }
}
fn parse_aac_asc_from_adts(adts: &[u8]) -> Option<[u8; 2]> {
    if adts.len() < 7 {
        return None;
    }

    // syncword 0xFFF
    if adts[0] != 0xFF || (adts[1] & 0xF0) != 0xF0 {
        return None;
    }

    let profile = ((adts[2] & 0xC0) >> 6) + 1;
    let sf_index = (adts[2] & 0x3C) >> 2;
    let chan_cfg = ((adts[2] & 0x01) << 2) | ((adts[3] & 0xC0) >> 6);

    let asc0 = (profile << 3) | (sf_index >> 1);
    let asc1 = ((sf_index & 1) << 7) | (chan_cfg << 3);

    Some([asc0, asc1])
}
fn extract_aac_asc(pkt: &AVPacket) -> Option<[u8; 2]> {
    unsafe {
        let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);
        parse_aac_asc_from_adts(data)
    }
}
unsafe fn repair_codecpar(
    stream: *mut AVStream,
    pkt: &AVPacket,
    param: &mut ParamRepairState,
) -> bool {
    let codecpar = (*stream).codecpar;
    debug!(
        "codec_id(enum)={} codec_tag={} extradata_size={}",
        (*codecpar).codec_id,
        (*codecpar).codec_tag,
        (*codecpar).extradata_size
    );

    match (*codecpar).codec_id {
        AVCodecID_AV_CODEC_ID_H264 => {
            // // 打印当前extradata状态
            // if !(*codecpar).extradata.is_null() && (*codecpar).extradata_size > 0 {
            //     let size = (*codecpar).extradata_size as usize;
            //     let slice = std::slice::from_raw_parts((*codecpar).extradata, size.min(32));
            //     debug!(
            //         "Current H264 extradata (first {} of {}): {:02X?}",
            //         slice.len(),
            //         size,
            //         slice
            //     );
            // }

            // 修复 H264 PS
            let ps = param.h264_ps.get_or_insert_with(Default::default);
            extract_h264_ps(pkt, ps);

            if ps.sps.is_none() || ps.pps.is_none() {
                debug!("H264: Waiting for SPS/PPS");
                return false;
            }

            let sps = ps.sps.as_ref().unwrap();
            let pps = ps.pps.as_ref().unwrap();

            println!("H264 SPS ({} bytes): {:02X?}", sps.len(), sps);
            println!("H264 PPS ({} bytes): {:02X?}", pps.len(), pps);

            let extradata_size = 4 + sps.len() + 4 + pps.len();
            let extradata = av_malloc(extradata_size) as *mut u8;

            // 验证内存分配
            if extradata.is_null() {
                error!("Failed to allocate {} bytes for extradata", extradata_size);
                return false;
            }

            // 填充 extradata
            let mut offset = 0;
            for nal in [sps, pps] {
                ptr::copy_nonoverlapping([0, 0, 0, 1].as_ptr(), extradata.add(offset), 4);
                offset += 4;
                ptr::copy_nonoverlapping(nal.as_ptr(), extradata.add(offset), nal.len());
                offset += nal.len();
            }

            // 验证填充大小
            if offset != extradata_size {
                error!(
                    "Extradata size mismatch: expected {}, got {}",
                    extradata_size, offset
                );
                av_free(extradata as *mut _);
                return false;
            }

            // 打印新建的extradata
            let new_extradata_slice = std::slice::from_raw_parts(extradata, extradata_size);
            debug!(
                "New H264 AnnexB extradata ({} bytes): {:02X?}",
                extradata_size, new_extradata_slice
            );

            // 释放旧 extradata
            if !(*codecpar).extradata.is_null() {
                debug!("Freeing old extradata at {:p}", (*codecpar).extradata);
                av_free((*codecpar).extradata as *mut _);
            }

            (*codecpar).extradata = extradata;
            (*codecpar).extradata_size = extradata_size as i32;

            debug!(
                "H264 extradata updated: ptr={:p}, size={}",
                (*codecpar).extradata,
                (*codecpar).extradata_size
            );
            true
        }

        AVCodecID_AV_CODEC_ID_HEVC => {
            // 修复 H265 PS
            let ps = param.h265_ps.get_or_insert_with(Default::default);
            extract_h265_ps(pkt, ps);

            if ps.vps.is_none() || ps.sps.is_none() || ps.pps.is_none() {
                return false;
            }

            let vps = ps.vps.as_ref().unwrap();
            let sps = ps.sps.as_ref().unwrap();
            let pps = ps.pps.as_ref().unwrap();

            let extradata_size = 4 + vps.len() + 4 + sps.len() + 4 + pps.len();
            let extradata = av_malloc(extradata_size) as *mut u8;
            let mut offset = 0;

            for nal in [vps, sps, pps] {
                ptr::copy_nonoverlapping([0, 0, 0, 1].as_ptr(), extradata.add(offset), 4);
                offset += 4;
                ptr::copy_nonoverlapping(nal.as_ptr(), extradata.add(offset), nal.len());
                offset += nal.len();
            }

            // 验证填充大小
            if offset != extradata_size {
                error!(
                    "Extradata size mismatch: expected {}, got {}",
                    extradata_size, offset
                );
                av_free(extradata as *mut _);
                return false;
            }

            if !(*codecpar).extradata.is_null() {
                av_free((*codecpar).extradata as *mut _);
            }

            (*codecpar).extradata = extradata;
            (*codecpar).extradata_size = extradata_size as i32;
            true
        }

        AVCodecID_AV_CODEC_ID_AAC => {
            // 修复 AAC ASC
            match param.aac_asc {
                None => {
                    if let Some(asc) = extract_aac_asc(pkt) {
                        param.aac_asc = Some(asc);

                        if !(*codecpar).extradata.is_null() {
                            av_free((*codecpar).extradata as *mut _);
                        }

                        (*codecpar).extradata = av_malloc(2) as *mut u8;
                        (*codecpar).extradata_size = 2;
                        ptr::copy_nonoverlapping(asc.as_ptr(), (*codecpar).extradata, 2);
                        true
                    } else {
                        false
                    }
                }
                Some(_) => true,
            }
        }
        _ => true,
    }
}
/// 修复音频流信息
unsafe fn repair_audio_stream_info(stream: *mut AVStream, media_ext: &MediaExt) {
    let par = (*stream).codecpar;

    debug!(
        "Repairing audio stream info for codec_id: {}",
        (*par).codec_id
    );

    // === 1. 修复采样率 ===
    if (*par).sample_rate <= 0 {
        if let Some(ref sr_str) = media_ext.audio_params.sample_rate {
            if let Ok(mut sr) = sr_str.parse::<i32>() {
                if sr > 0 && sr < 1000 {
                    sr *= 1000;
                } // 处理kHz单位
                (*par).sample_rate = sr;
                debug!("Set sample_rate from media_ext: {} Hz", sr);
            }
        }
        if (*par).sample_rate <= 0 {
            (*par).sample_rate = 8000;
            debug!("Set sample_rate from default: {} Hz", 8000);
        }
    }

    // === 2. 修复声道数 ===
    if (*par).channels <= 0 {
        //设置默认值
        (*par).channels = 1;
    }

    // === 3. 修复声道布局 ===
    if (*par).channel_layout == 0 {
        (*par).channel_layout = 4;
    }

    // === 4. 修复采样格式 ===
    if (*par).format == -1 {
        // AV_SAMPLE_FMT_NONE
        match (*par).codec_id {
            AVCodecID_AV_CODEC_ID_AAC => {
                (*par).format = AVSampleFormat_AV_SAMPLE_FMT_FLTP as i32;
                debug!("Set AAC sample format to FLTP");
            }
            AVCodecID_AV_CODEC_ID_PCM_ALAW | AVCodecID_AV_CODEC_ID_PCM_MULAW => {
                (*par).format = AVSampleFormat_AV_SAMPLE_FMT_S16 as i32;
                debug!("Set G.711 sample format to S16");
            }
            _ => {
                (*par).format = AVSampleFormat_AV_SAMPLE_FMT_S16 as i32;
                debug!("Set default sample format to S16");
            }
        }
    }

    // === 5. 修复码率 ===
    if (*par).bit_rate <= 0 {
        if let Some(ref br_str) = media_ext.audio_params.bitrate {
            if let Ok(br_kbps) = br_str.parse::<i64>() {
                (*par).bit_rate = br_kbps * 1000;
                debug!("Set audio bitrate from media_ext: {} kbps", br_kbps);
            }
        }

        // 如果没有设置，根据编码格式估算
        if (*par).bit_rate <= 0 {
            let estimated_rate = match (*par).codec_id {
                AVCodecID_AV_CODEC_ID_AAC => {
                    match (*par).sample_rate {
                        8000 => 12000,  // 8kHz AAC
                        16000 => 24000, // 16kHz AAC
                        44100 => 64000, // 44.1kHz AAC
                        48000 => 96000, // 48kHz AAC
                        _ => 64000,     // 默认64kbps
                    }
                }
                AVCodecID_AV_CODEC_ID_PCM_ALAW | AVCodecID_AV_CODEC_ID_PCM_MULAW => {
                    64000 // G.711是64kbps
                }
                _ => {
                    128000 // 默认128kbps
                }
            };

            (*par).bit_rate = estimated_rate;
            debug!("Estimated audio bitrate: {} bps", estimated_rate);
        }
    }
}

/*
/// 修复视频流信息
unsafe fn repair_video_stream_info(
    stream: *mut AVStream,
    param: &ParamRepairState,
    media_ext: Option<&MediaExt>,
) {
    let par = (*stream).codecpar;

    debug!(
        "Repairing video stream info for codec_id: {}",
        (*par).codec_id
    );

    // === 1. 修复分辨率 ===
    if (*par).width <= 0 || (*par).height <= 0 {
        // 尝试从SPS解析分辨率 (对于H.264/H.265)
        if ((*par).width <= 0 || (*par).height <= 0)
            && ((*par).codec_id == AVCodecID_AV_CODEC_ID_H264
                || (*par).codec_id == AVCodecID_AV_CODEC_ID_HEVC)
        {
            if let Some((width, height)) = parse_resolution_from_sps(param, (*par).codec_id) {
                (*par).width = width;
                (*par).height = height;
                debug!("Parsed resolution from packet: {}x{}", width, height);
            }
        }

        if let Some(ext) = media_ext {
            if let Some((w, h)) = ext.video_params.resolution {
                (*par).width = w;
                (*par).height = h;
                debug!("Set resolution from media_ext: {}x{}", w, h);
            }
        }

        if (*par).width <= 0 || (*par).height <= 0 {
            //todo err
            warn!("Missing resolution for video stream");
        }
    }

    // === 2. 修复像素格式 ===
    if (*par).format == -1 {
        // AV_PIX_FMT_NONE
        // H.264/H.265通常使用YUV420P，若画面灰暗则YUVJ420P
        (*par).format = AVPixelFormat_AV_PIX_FMT_YUV420P as i32;
        info!("Fix Set pixel format to YUV420P for H.264/H.265");
    }

    // === 3. 修复码率 ===
    if (*par).bit_rate <= 0 {
        if let Some(ext) = media_ext {
            if let Some(br_kbps) = ext.video_params.bitrate {
                (*par).bit_rate = (br_kbps as i64) * 1000;
                debug!("Set bitrate from media_ext: {} kbps", br_kbps);
            }
        }
        if (*par).bit_rate <= 0 {
            (*par).bit_rate = 1024;
            debug!("Set default bitrate: {} bps", 1024);
        }
    }

    // === 4. 修复 profile/level (从SPS解析) ===
    if (*par).profile == 0 || (*par).level == 0 {
        if (*par).codec_id == AVCodecID_AV_CODEC_ID_H264 {
            if let Some((profile, level)) = parse_h264_profile_level_from_packet(param) {
                if (*par).profile == 0 {
                    (*par).profile = profile;
                }
                if (*par).level == 0 {
                    (*par).level = level;
                }
                debug!("Parsed H.264 profile={}, level={}", profile, level);
            }
        } else if (*par).codec_id == AVCodecID_AV_CODEC_ID_HEVC {
            if let Some((profile, level)) = parse_h265_profile_level_from_packet(param) {
                if (*par).profile == 0 {
                    (*par).profile = profile;
                }
                if (*par).level == 0 {
                    (*par).level = level;
                }
                debug!("Parsed H.265 profile={}, level={}", profile, level);
            }
        }
    }
}


/// 从数据包解析分辨率 (用于H.264/H.265)
unsafe fn parse_resolution_from_sps(param: &ParamRepairState, codec_id: u32) -> Option<(i32, i32)> {
    match codec_id {
        AVCodecID_AV_CODEC_ID_H264 => {
            // SPS
            if let Some(H264ParameterSets { sps: Some(sps), .. }) = &param.h264_ps {
                if let Some((width, height)) = parse_h264_sps_dimensions(sps) {
                    return Some((width, height));
                }
            }
        }
        AVCodecID_AV_CODEC_ID_HEVC => {
            if let Some(H265ParameterSets { sps: Some(sps), .. }) = &param.h265_ps {
                if let Some((width, height)) = parse_h265_sps_dimensions(sps) {
                    return Some((width, height));
                }
            }
        }
        _ => {}
    }
    Some((1920, 1080))
}

/// 解析H.264 SPS获取分辨率
fn parse_h264_sps_dimensions(data: &[u8]) -> Option<(i32, i32)> {
    if data.len() < 7 {
        return None;
    }

    let profile_idc = data[0];
    let constraint_flags = data[1];
    let level_idc = data[2];
    let seq_parameter_set_id = data[3] & 0x1F;

    let mut offset = 4;

    // 解析chroma_format_idc
    let chroma_format_idc = if profile_idc == 100
        || profile_idc == 110
        || profile_idc == 122
        || profile_idc == 244
        || profile_idc == 44
        || profile_idc == 83
        || profile_idc == 86
        || profile_idc == 118
        || profile_idc == 128
        || profile_idc == 138
        || profile_idc == 139
        || profile_idc == 134
        || profile_idc == 135
    {
        let chroma = data[offset] & 0x03;
        offset += 1;
        if chroma == 3 {
            offset += 1; // separate_colour_plane_flag
        }
        chroma
    } else {
        0
    };

    // 解析bit_depth_luma_minus8
    if profile_idc == 100
        || profile_idc == 110
        || profile_idc == 122
        || profile_idc == 244
        || profile_idc == 44
        || profile_idc == 83
        || profile_idc == 86
        || profile_idc == 118
        || profile_idc == 128
        || profile_idc == 138
        || profile_idc == 139
        || profile_idc == 134
        || profile_idc == 135
    {
        offset += 1; // bit_depth_luma_minus8
        offset += 1; // bit_depth_chroma_minus8
        offset += 1; // qpprime_y_zero_transform_bypass_flag
        let seq_scaling_matrix_present_flag = data[offset] & 0x01;
        offset += 1;
        if seq_scaling_matrix_present_flag != 0 {
            // 跳过scaling lists
            for i in 0..8 {
                let seq_scaling_list_present_flag = if i < 6 {
                    (data[offset] >> (7 - (i % 8))) & 0x01
                } else {
                    (data[offset] >> (13 - i)) & 0x01
                };
                if seq_scaling_list_present_flag != 0 {
                    // TODO: 解析scaling list
                    return None;
                }
            }
        }
    }

    // 解析log2_max_frame_num_minus4
    let log2_max_frame_num_minus4 = data[offset] & 0x0F;
    offset += 1;

    // 解析pic_order_cnt_type
    let pic_order_cnt_type = data[offset] & 0x03;
    offset += 1;

    if pic_order_cnt_type == 0 {
        let log2_max_pic_order_cnt_lsb_minus4 = data[offset] & 0x0F;
        offset += 1;
    } else if pic_order_cnt_type == 1 {
        let delta_pic_order_always_zero_flag = data[offset] & 0x01;
        offset += 1;
        let offset_for_non_ref_pic = data[offset] as i32;
        offset += 1;
        let offset_for_top_to_bottom_field = data[offset] as i32;
        offset += 1;
        let num_ref_frames_in_pic_order_cnt_cycle = data[offset] as i32;
        offset += 1;
        for _ in 0..num_ref_frames_in_pic_order_cnt_cycle {
            offset += 1; // offset_for_ref_frame[i]
        }
    }

    // 解析max_num_ref_frames
    let max_num_ref_frames = data[offset] as i32;
    offset += 1;

    // 解析gaps_in_frame_num_value_allowed_flag
    let gaps_in_frame_num_value_allowed_flag = data[offset] & 0x01;
    offset += 1;

    // 解析pic_width_in_mbs_minus1
    if offset + 4 > data.len() {
        return None;
    }

    let pic_width_in_mbs_minus1 = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
    offset += 2;

    // 解析pic_height_in_map_units_minus1
    let pic_height_in_map_units_minus1 = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
    offset += 2;

    // 计算分辨率
    let width = ((pic_width_in_mbs_minus1 as i32) + 1) * 16;
    let height = ((pic_height_in_map_units_minus1 as i32) + 1) * 16;

    // 检查frame_mbs_only_flag
    let frame_mbs_only_flag = (data[offset] >> 7) & 0x01;
    if frame_mbs_only_flag == 0 {
        // 场模式，高度需要乘以2
        // let height = height * 2;
        // 暂时不处理场模式
    }

    Some((width, height))
}

/// 解析H.265 SPS获取分辨率 (简化版本)
fn parse_h265_sps_dimensions(_data: &[u8]) -> Option<(i32, i32)> {
    // H.265 SPS解析比较复杂，这里返回None，让上层从其他地方获取
    // 实际项目中可以实现完整的H.265 SPS解析
    None
}

/// 从数据包解析H.264 profile和level
fn parse_h264_profile_level_from_packet(param: &ParamRepairState) -> Option<(i32, i32)> {
    // SPS
    if let Some(H264ParameterSets { sps: Some(sps), .. }) = &param.h264_ps {
        let profile_idc = sps[1] as i32;
        let level_idc = sps[3] as i32;
        return Some((profile_idc, level_idc));
    }
    Some((66, 40))
}

/// 从数据包解析H.265 profile和level
fn parse_h265_profile_level_from_packet(param: &ParamRepairState) -> Option<(i32, i32)> {
    if let Some(H265ParameterSets { sps: Some(vps), .. }) = &param.h265_ps {
        if vps.len() < 13 {
            return Some((1, 120));
        }
        // Skip NAL header (1 byte)
        let rbsp = &vps[1..];

        // Remove emulation prevention bytes (0x00 0x00 0x03 -> 0x00 0x00)
        let mut clean = Vec::with_capacity(rbsp.len());
        let mut i = 0;
        while i < rbsp.len() {
            if i + 2 < rbsp.len() && rbsp[i] == 0 && rbsp[i + 1] == 0 && rbsp[i + 2] == 3 {
                clean.push(0);
                clean.push(0);
                i += 3;
            } else {
                clean.push(rbsp[i]);
                i += 1;
            }
        }
        if clean.len() < 12 {
            return None;
        }
        // profile_idc is in byte[1], bits 3..7 (high 5 bits)
        let profile_idc = (clean[1] >> 3) & 0x1F;
        // level_idc is in byte[11]
        let level_idc = clean[11];
        return Some((profile_idc as i32, level_idc as i32));
    }
    Some((1, 120))
}

/// 辅助函数：在数据包中查找PCR值 (用于PS流)
pub unsafe fn find_pcr_in_packet(pkt: &AVPacket) -> Option<i64> {
    let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);

    // PS流中PCR在pack_header中: 0x000001BA开头
    if data.len() >= 14 && data[0..4] == [0x00, 0x00, 0x01, 0xBA] {
        // pack_header结构
        // bytes 4-9: system_clock_reference_base [33 bits]和system_clock_reference_extension [9 bits]
        let mut pcr_base: i64 = 0;
        pcr_base |= ((data[4] as i64) << 25) & 0x1FE000000;
        pcr_base |= ((data[5] as i64) << 17) & 0x1FE0000;
        pcr_base |= ((data[6] as i64) << 9) & 0x1FE00;
        pcr_base |= ((data[7] as i64) << 1) & 0x1FE;
        pcr_base |= ((data[8] as i64) >> 7) & 0x01;

        let pcr_ext = (((data[8] as i64) << 8) | (data[9] as i64)) & 0x1FF;

        // PCR = pcr_base * 300 + pcr_ext
        let pcr = pcr_base * 300 + pcr_ext;

        debug!(
            "Found PCR in PS packet: base={}, ext={}, total={}",
            pcr_base, pcr_ext, pcr
        );
        return Some(pcr);
    }

    None
}

*/
