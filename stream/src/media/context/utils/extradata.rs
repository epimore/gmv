use base::chrono;
use crate::general::mp::{Audio, MediaParam, Video};
use crate::media::context::format::demuxer::DemuxerContext;
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,AVCodecID_AV_CODEC_ID_OPUS
};
pub fn parse_media_param(ctx: &DemuxerContext) -> MediaParam {
    let mut video: Option<Video> = None;
    let mut audio: Option<Audio> = None;

    unsafe {
        let fmt = ctx.avio.fmt_ctx;

        for (i, codecpar) in ctx.params.iter().enumerate().map(|(i, param)| (i, param.codecpar)) {
            let st = *(*fmt).streams.offset(i as isize);

            match (*codecpar).codec_type {
                rsmpeg::ffi::AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    if video.is_none() {
                        video = Some(parse_video(codecpar, st));
                    }
                }
                rsmpeg::ffi::AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    if audio.is_none() {
                        audio = Some(parse_audio(codecpar));
                    }
                }
                _ => {}
            }
        }
    }

    MediaParam {
        availability_start_time: chrono::Utc::now().to_rfc3339(),
        video,
        audio,
    }
}
unsafe fn parse_video(
    codecpar: *mut rsmpeg::ffi::AVCodecParameters,
    st: *mut rsmpeg::ffi::AVStream,
) -> Video {
    let width = (*codecpar).width as u32;
    let height = (*codecpar).height as u32;

    // ===== frame rate =====
    let fr = (*st).avg_frame_rate;
    let frame_rate = if fr.num > 0 && fr.den > 0 {
        (fr.num / fr.den).max(1) as u32
    } else {
        25
    };

    Video {
        codec: video_codec_string(codecpar),
        width,
        height,
        frame_rate,
        timescale: 1000,          // CMAF / DASH video 推荐
        bandwidth: estimate_video_bandwidth(width, height, frame_rate),
    }
}
unsafe fn parse_audio(codecpar: *mut rsmpeg::ffi::AVCodecParameters) -> Audio {
    let sample_rate = (*codecpar).sample_rate.max(1) as u32;
    let channels = (*codecpar).channels.max(1) as u32;

    Audio {
        codec: audio_codec_string(codecpar),
        sample_rate,
        channels,
        timescale: sample_rate,   // DASH 规范推荐
        bandwidth: estimate_audio_bandwidth(sample_rate, channels),
    }
}
unsafe fn video_codec_string(codecpar: *mut rsmpeg::ffi::AVCodecParameters) -> String {
    use rsmpeg::ffi::*;

    match (*codecpar).codec_id {
        AVCodecID_AV_CODEC_ID_H264 => {
            // extradata: AVCDecoderConfigurationRecord
            if (*codecpar).extradata.is_null() || (*codecpar).extradata_size < 7 {
                return "avc1.42E01E".into(); // fallback
            }

            let data = std::slice::from_raw_parts(
                (*codecpar).extradata,
                (*codecpar).extradata_size as usize,
            );

            let profile = data[1];
            let compat  = data[2];
            let level   = data[3];

            format!("avc1.{:02X}{:02X}{:02X}", profile, compat, level)
        }
        AVCodecID_AV_CODEC_ID_HEVC => "hev1.1.6.L120.B0".into(),
        _ => "unknown".into(),
    }
}
unsafe fn audio_codec_string(codecpar: *mut rsmpeg::ffi::AVCodecParameters) -> String {
    match (*codecpar).codec_id {
        AVCodecID_AV_CODEC_ID_AAC => "mp4a.40.2".into(),
        AVCodecID_AV_CODEC_ID_OPUS => "opus".into(),
        _ => "unknown".into(),
    }
}
fn estimate_video_bandwidth(w: u32, h: u32, fps: u32) -> u32 {
    let pixels = w * h;
    let bpp = 0.1; // bits per pixel per frame (经验值)
    ((pixels as f64 * fps as f64 * bpp) as u32).max(500_000)
}

fn estimate_audio_bandwidth(sample_rate: u32, channels: u32) -> u32 {
    (sample_rate * channels * 2 * 8).min(256_000)
}

use rsmpeg::ffi::*;
use std::ptr;
use rsmpeg::ffi::{AVERROR_EOF};

pub unsafe fn rebuild_codecpar_extradata_with_ffmpeg(
    in_par: *const AVCodecParameters,
    out_par: *mut AVCodecParameters,
) -> Result<(), i32> {
    let codec_id = (*in_par).codec_id;
    let codec = avcodec_find_decoder(codec_id);
    if codec.is_null() {
        return Err(AVERROR_DECODER_NOT_FOUND);
    }

    // 1. alloc codec ctx
    let codec_ctx = avcodec_alloc_context3(codec);
    if codec_ctx.is_null() {
        //内存不足
        return Err(-1);
    }

    // 2. copy parameters → codec ctx
    let ret = avcodec_parameters_to_context(codec_ctx, in_par);
    if ret < 0 {
        avcodec_free_context(&mut (codec_ctx as *mut _));
        return Err(ret);
    }

    // ⚠️ 关键点：不要设置 get_format / hwaccel
    // ⚠️ 只为了生成 extradata

    // 3. open codec（生成 extradata）
    let ret = avcodec_open2(codec_ctx, codec, ptr::null_mut());
    if ret < 0 {
        avcodec_free_context(&mut (codec_ctx as *mut _));
        return Err(ret);
    }

    // 4. codec ctx → out_par（生成 avcC / hvcC / ASC）
    let ret = avcodec_parameters_from_context(out_par, codec_ctx);

    avcodec_free_context(&mut (codec_ctx as *mut _));

    if ret < 0 {
        return Err(ret);
    }

    Ok(())
}
