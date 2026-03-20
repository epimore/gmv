use rsmpeg::ffi::*;
use log::{debug, info, warn};
use std::ptr;
use base::chrono;
use base::chrono::SecondsFormat;
use crate::general::mp::{Audio, MediaParam, Video};
use crate::media::context::format::demuxer::DemuxerContext;

/// 常见兼容的 H.264 codec 映射表
const H264_CODEC_MAP: &[(u32, &str)] = &[
    (66, "avc1.42E01E"),   // Baseline Profile, level 3.0
    (77, "avc1.4D0029"),   // Main Profile, level 4.1 (最常用)
    (100, "avc1.640028"),  // High Profile, level 4.0
    (110, "avc1.6E0028"),  // High 10 Profile
    (122, "avc1.7A0028"),  // High 4:2:2 Profile
    (144, "avc1.900028"),  // High 4:4:4 Profile
];

/// 默认的 H.264 codec (Main Profile level 4.1)
const DEFAULT_H264_CODEC: &str = "avc1.4D0029";

pub fn parse_media_param(ctx: &DemuxerContext) -> MediaParam {
    let mut video: Option<Video> = None;
    let mut audio: Option<Audio> = None;

    unsafe {
        let fmt = ctx.avio.fmt_ctx;
        for i in 0..(*fmt).nb_streams {
            let st = *(*fmt).streams.offset(i as isize);
            let codecpar = (*st).codecpar;

            match (*codecpar).codec_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    if video.is_none() {
                        match parse_video(codecpar, st) {
                            Ok(v) => video = Some(v),
                            Err(e) => warn!("Failed to parse video stream {}: {}", i, e),
                        }
                    }
                }
                AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    if audio.is_none() {
                        match parse_audio(codecpar) {
                            Ok(a) => audio = Some(a),
                            Err(e) => warn!("Failed to parse audio stream {}: {}", i, e),
                        }
                    }
                }
                _ => {}
            }
        }
    }

    MediaParam {
        start_time: chrono::Local::now(),
        video,
        audio,
    }
}

unsafe fn parse_video(
    codecpar: *mut AVCodecParameters,
    st: *mut AVStream,
) -> Result<Video, String> {
    // === 1. 获取宽高 ===
    let width = (*codecpar).width;
    let height = (*codecpar).height;

    if width <= 0 || height <= 0 {
        return Err(format!("Invalid video dimensions: {}x{}", width, height));
    }
    let width = width as u32;
    let height = height as u32;

    // === 2. 获取帧率（多种方式） ===
    let frame_rate = get_video_frame_rate(st, codecpar).unwrap_or(25);

    // === 3. 获取 codec 字符串（已修正） ===
    let codec = get_video_codec_string(codecpar)?;

    // === 4. 验证并修正 codec 字符串 ===
    let codec = normalize_h264_codec(&codec);

    // === 5. 估算带宽 ===
    let bandwidth = estimate_video_bandwidth(width, height, frame_rate);

    debug!(
        "Video parsed: {}x{}, {} fps, codec={}, bandwidth={}",
        width, height, frame_rate, codec, bandwidth
    );

    Ok(Video {
        codec,
        width,
        height,
        frame_rate,
        timescale: 90000,  // 视频推荐使用 90000
        bandwidth,
    })
}

unsafe fn parse_audio(codecpar: *mut AVCodecParameters) -> Result<Audio, String> {
    // === 1. 获取采样率 ===
    let sample_rate = (*codecpar).sample_rate;
    if sample_rate <= 0 {
        return Err(format!("Invalid sample rate: {}", sample_rate));
    }
    let sample_rate = sample_rate as u32;

    // === 2. 获取声道数 ===
    let channels = (*codecpar).channels;
    if channels <= 0 {
        return Err(format!("Invalid channels: {}", channels));
    }
    let channels = channels as u32;

    // === 3. 获取 codec 字符串 ===
    let codec = get_audio_codec_string(codecpar)?;

    // === 4. 估算带宽 ===
    let bandwidth = estimate_audio_bandwidth(sample_rate, channels);

    debug!(
        "Audio parsed: {} Hz, {} ch, codec={}, bandwidth={}",
        sample_rate, channels, codec, bandwidth
    );

    Ok(Audio {
        codec,
        sample_rate,
        channels,
        timescale: sample_rate,  // 音频 timescale 使用采样率
        bandwidth,
    })
}

/// 获取视频帧率（多种 fallback 机制）
unsafe fn get_video_frame_rate(st: *mut AVStream, codecpar: *mut AVCodecParameters) -> Option<u32> {
    // 方法1: 从流的 avg_frame_rate 获取
    let fr = (*st).avg_frame_rate;
    if fr.num > 0 && fr.den > 0 {
        let fps = (fr.num as f64 / fr.den as f64).round() as u32;
        if fps > 0 && fps <= 120 {
            debug!("Using avg_frame_rate: {}/{} = {} fps", fr.num, fr.den, fps);
            return Some(fps);
        }
    }

    // 方法2: 从流的 r_frame_rate 获取
    let fr = (*st).r_frame_rate;
    if fr.num > 0 && fr.den > 0 {
        let fps = (fr.num as f64 / fr.den as f64).round() as u32;
        if fps > 0 && fps <= 120 {
            debug!("Using r_frame_rate: {}/{} = {} fps", fr.num, fr.den, fps);
            return Some(fps);
        }
    }

    // 方法3: 从 codecpar 的 framerate 获取（某些格式会设置）
    if (*codecpar).framerate.num > 0 && (*codecpar).framerate.den > 0 {
        let fps = ((*codecpar).framerate.num as f64 / (*codecpar).framerate.den as f64).round() as u32;
        if fps > 0 && fps <= 120 {
            debug!("Using codecpar framerate: {}/{} = {} fps",
                (*codecpar).framerate.num, (*codecpar).framerate.den, fps);
            return Some(fps);
        }
    }

    // 方法4: 对于 H.264/H.265，使用默认监控设备帧率
    if (*codecpar).codec_id == AVCodecID_AV_CODEC_ID_H264
        || (*codecpar).codec_id == AVCodecID_AV_CODEC_ID_HEVC {
        debug!("Using default surveillance fps: 25");
        return Some(25);
    }

    None
}

/// 获取视频 codec 字符串（支持多种格式）
unsafe fn get_video_codec_string(codecpar: *mut AVCodecParameters) -> Result<String, String> {
    match (*codecpar).codec_id {
        AVCodecID_AV_CODEC_ID_H264 => get_h264_codec_string(codecpar),
        AVCodecID_AV_CODEC_ID_HEVC => get_hevc_codec_string(codecpar),
        _ => Err(format!("Unsupported video codec: {}", (*codecpar).codec_id)),
    }
}

/// 获取 H.264 codec 字符串（修正版）
unsafe fn get_h264_codec_string(codecpar: *mut AVCodecParameters) -> Result<String, String> {
    // 方法1: 直接从 codecpar 的 profile/level 获取（最可靠）
    let profile = (*codecpar).profile;
    let level = (*codecpar).level;

    if profile > 0 && level > 0 {
        // 根据 profile 选择对应的标准 codec 字符串
        for &(p, codec) in H264_CODEC_MAP {
            if p == profile as u32 {
                debug!("H.264 codec from profile map: profile={}, codec={}", profile, codec);
                return Ok(codec.to_string());
            }
        }

        // 如果没有精确匹配，根据 profile 范围选择
        let codec = match profile {
            66..=76 => "avc1.42E01E",   // Baseline 范围
            77..=99 => "avc1.4D0029",   // Main 范围
            100..=109 => "avc1.640028", // High 范围
            _ => DEFAULT_H264_CODEC,
        };
        debug!("H.264 codec from profile range: profile={}, codec={}", profile, codec);
        return Ok(codec.to_string());
    }

    // 方法2: 从 extradata 解析（需要验证格式）
    if (*codecpar).extradata.is_null() || (*codecpar).extradata_size < 8 {
        warn!("No valid extradata, using default H.264 codec");
        return Ok(DEFAULT_H264_CODEC.to_string());
    }

    let data = std::slice::from_raw_parts(
        (*codecpar).extradata,
        (*codecpar).extradata_size as usize,
    );

    // 判断 extradata 格式
    if data.len() >= 8 && data[0] == 1 {
        // AVCC 格式
        let profile = data[1];
        // let level = data[3];

        // 根据 profile 和 level 选择标准 codec
        for &(p, codec) in H264_CODEC_MAP {
            if p == profile as u32 {
                debug!("H.264 codec from AVCC profile: {}", codec);
                return Ok(codec.to_string());
            }
        }

        // 如果找不到精确匹配，使用默认
        debug!("Using default H.264 codec from AVCC");
        Ok(DEFAULT_H264_CODEC.to_string())
    } else if data.len() >= 4 && (data[0..4] == [0, 0, 0, 1] || data[0..3] == [0, 0, 1]) {
        // AnnexB 格式，尝试解析
        match parse_h264_from_annexb(data) {
            Ok(codec) => Ok(codec),
            Err(_) => {
                warn!("Failed to parse AnnexB, using default");
                Ok(DEFAULT_H264_CODEC.to_string())
            }
        }
    } else {
        warn!("Unknown extradata format, using default");
        Ok(DEFAULT_H264_CODEC.to_string())
    }
}

/// 从 AnnexB 格式的 extradata 解析 H.264 codec
unsafe fn parse_h264_from_annexb(data: &[u8]) -> Result<String, String> {
    let mut i = 0;
    while i + 4 < data.len() {
        // 查找 start code
        if data[i..].starts_with(&[0, 0, 0, 1]) || data[i..].starts_with(&[0, 0, 1]) {
            let start_len = if data[i..].starts_with(&[0, 0, 0, 1]) { 4 } else { 3 };
            let nalu_start = i + start_len;

            // 找到 NALU 类型
            if nalu_start < data.len() {
                let nal_type = data[nalu_start] & 0x1F;

                // SPS (nal_type == 7)
                if nal_type == 7 && nalu_start + 3 < data.len() {
                    let profile = data[nalu_start + 1];

                    // 根据 profile 选择标准 codec
                    for &(p, codec) in H264_CODEC_MAP {
                        if p == profile as u32 {
                            debug!("H.264 codec from AnnexB SPS profile: {}", codec);
                            return Ok(codec.to_string());
                        }
                    }
                }
            }

            // 跳到下一个 NALU
            i = nalu_start;
            while i < data.len() && data[i] != 0 {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    Err("No valid SPS found".to_string())
}

/// 标准化 H.264 codec 字符串（确保格式正确）
fn normalize_h264_codec(codec: &str) -> String {
    // 如果已经是标准格式，直接返回
    for &(_, standard_codec) in H264_CODEC_MAP {
        if codec == standard_codec {
            return codec.to_string();
        }
    }

    // 尝试提取 profile 信息
    if codec.starts_with("avc1.") && codec.len() >= 10 {
        let profile_hex = &codec[5..7];

        // 将 hex 转换为数字
        if let Ok(profile) = u32::from_str_radix(profile_hex, 16) {
            for &(p, standard_codec) in H264_CODEC_MAP {
                if p == profile {
                    debug!("Normalized codec {} -> {}", codec, standard_codec);
                    return standard_codec.to_string();
                }
            }
        }
    }

    // 检查是否为常见的错误格式
    if codec == "avc1.4D0E029" {
        debug!("Fixed known bad codec: {} -> {}", codec, DEFAULT_H264_CODEC);
        return DEFAULT_H264_CODEC.to_string();
    }

    // 默认返回
    debug!("Using default codec for: {}", codec);
    DEFAULT_H264_CODEC.to_string()
}

/// 获取 HEVC codec 字符串
unsafe fn get_hevc_codec_string(codecpar: *mut AVCodecParameters) -> Result<String, String> {
    let profile = (*codecpar).profile;
    let level = (*codecpar).level;

    if profile > 0 && level > 0 {
        let profile_space = match profile {
            1 => "1",   // Main
            2 => "2",   // Main 10
            3 => "3",   // Main Still Picture
            _ => "1",
        };

        let tier = if (*codecpar).profile == 2 { "H" } else { "L" };
        let codec = format!("hev1.{}.6.{}{:02X}", profile_space, tier, level);
        debug!("HEVC codec from profile/level: {}", codec);
        Ok(codec)
    } else {
        // 默认 HEVC Main Profile
        Ok("hev1.1.6.L120".into())
    }
}

/// 获取音频 codec 字符串
unsafe fn get_audio_codec_string(codecpar: *mut AVCodecParameters) -> Result<String, String> {
    match (*codecpar).codec_id {
        AVCodecID_AV_CODEC_ID_AAC => {
            // AAC 有多种 profile
            let profile = (*codecpar).profile;
            let aac_profile = match profile {
                1 => "2",   // AAC LC
                2 => "5",   // AAC HE (SBR)
                3 => "5",   // AAC HE v2
                4 => "29",  // AAC LD
                5 => "39",  // AAC ELD
                _ => "2",   // 默认 AAC LC
            };

            Ok(format!("mp4a.40.{}", aac_profile))
        }
        AVCodecID_AV_CODEC_ID_OPUS => Ok("opus".into()),
        AVCodecID_AV_CODEC_ID_PCM_ALAW => Ok("alaw".into()),
        AVCodecID_AV_CODEC_ID_PCM_MULAW => Ok("mulaw".into()),
        AVCodecID_AV_CODEC_ID_MP3 => Ok("mp4a.40.34".into()), // MP3
        _ => Err(format!("Unsupported audio codec: {}", (*codecpar).codec_id)),
    }
}

/// 估算视频带宽（基于分辨率、帧率和编码效率）
fn estimate_video_bandwidth(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = width * height;
    let bitrate = match pixels {
        p if p >= 1920 * 1080 => { // 1080p
            match fps {
                f if f >= 50 => 8_000_000,  // 8 Mbps for 50fps+
                f if f >= 25 => 4_000_000,  // 4 Mbps for 25-30fps
                _ => 2_000_000,
            }
        }
        p if p >= 1280 * 720 => { // 720p
            match fps {
                f if f >= 50 => 4_000_000,
                f if f >= 25 => 2_500_000,
                _ => 1_500_000,
            }
        }
        p if p >= 720 * 576 => { // D1
            match fps {
                f if f >= 25 => 1_500_000,
                _ => 1_000_000,
            }
        }
        p if p >= 352 * 288 => { // CIF
            512_000
        }
        _ => 384_000,
    };

    bitrate.max(500_000).min(20_000_000) // 限制范围
}

/// 估算音频带宽
fn estimate_audio_bandwidth(sample_rate: u32, channels: u32) -> u32 {
    match sample_rate {
        sr if sr >= 48000 => {
            if channels >= 2 {
                192_000  // 48kHz stereo
            } else {
                96_000   // 48kHz mono
            }
        }
        sr if sr >= 44100 => {
            if channels >= 2 {
                128_000  // 44.1kHz stereo
            } else {
                64_000   // 44.1kHz mono
            }
        }
        sr if sr >= 16000 => 32_000,
        _ => 24_000,
    }.min(256_000)
}

/// 调试辅助：打印 codec 信息
#[allow(dead_code)]
pub fn debug_codec_info(codec: &str) {
    let supported = is_codec_supported(codec);
    debug!("Codec: {} - {}", codec, if supported { "✅" } else { "❌" });
}

/// 检查 codec 是否被支持（模拟 MediaSource.isTypeSupported）
fn is_codec_supported(codec: &str) -> bool {
    // 常见的支持 codec 列表
    let supported_codecs = [
        "avc1.42E01E", // Baseline 3.0
        "avc1.4D401E", // Main 3.0
        "avc1.4D0029", // Main 4.1
        "avc1.640028", // High 4.0
        "avc1.64001F", // High 3.1
        "mp4a.40.2",   // AAC LC
        "mp4a.40.5",   // AAC HE
    ];

    supported_codecs.contains(&codec)
}

/// 调试辅助：打印完整的流信息
#[allow(dead_code)]
pub unsafe fn dump_stream_info(ctx: &DemuxerContext) {
    let fmt = ctx.avio.fmt_ctx;
    info!("=== Media Stream Info ===");

    for i in 0..(*fmt).nb_streams {
        let st = *(*fmt).streams.add(i as usize);
        let codecpar = (*st).codecpar;

        match (*codecpar).codec_type {
            AVMediaType_AVMEDIA_TYPE_VIDEO => {
                info!("Video Stream #{}:", i);
                info!("  Codec ID: {}", (*codecpar).codec_id);
                info!("  Resolution: {}x{}", (*codecpar).width, (*codecpar).height);
                info!("  Profile: {}", (*codecpar).profile);
                info!("  Level: {}", (*codecpar).level);
                info!("  format: {}", (*codecpar).format);
                info!("  avg_frame_rate: {}/{}", (*st).avg_frame_rate.num, (*st).avg_frame_rate.den);
                info!("  r_frame_rate: {}/{}", (*st).r_frame_rate.num, (*st).r_frame_rate.den);
                info!("  time_base: {}/{}", (*st).time_base.num, (*st).time_base.den);
                info!("  duration: {}", (*st).duration);
                info!("  start time: {}", (*st).start_time);
                info!("  Extradata size: {}", (*codecpar).extradata_size);
            }
            AVMediaType_AVMEDIA_TYPE_AUDIO => {
                info!("Audio Stream #{}:", i);
                info!("  Codec ID: {}", (*codecpar).codec_id);
                info!("  Sample Rate: {} Hz", (*codecpar).sample_rate);
                info!("  Channels: {}", (*codecpar).channels);
                info!("  Profile: {}", (*codecpar).profile);
                info!("  time_base: {}/{}", (*st).time_base.num, (*st).time_base.den);
            }
            _ => {}
        }
    }
}