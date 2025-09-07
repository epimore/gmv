use rsmpeg::ffi::*;
use std::ffi::CStr;

/// 通用 extradata 修复
///
/// - H.264: 从 IDR 提取 SPS/PPS 更新 extradata
/// - H.265: 提取 VPS/SPS/PPS
/// - MPEG4: 用 FFmpeg bsf 生成 global headers
/// - AAC: 用 aac_adtstoasc 生成 AudioSpecificConfig
/// - G.711/G.722/G.723/G.729: 无需 extradata，直接跳过
pub unsafe fn fix_extradata(pkt: *mut AVPacket, st: *mut AVStream) -> i32 {
    if pkt.is_null() || st.is_null() {
        return -1;
    }
    let codecpar = (*st).codecpar;
    if codecpar.is_null() {
        return -1;
    }

    match (*codecpar).codec_id {
        // H.264
        AVCodecID_AV_CODEC_ID_H264 => {
            if is_idr(pkt) {
                return extract_and_update(codecpar, pkt, "h264_mp4toannexb\0");
            }
        }
        // H.265 / HEVC
        AVCodecID_AV_CODEC_ID_HEVC => {
            if is_idr(pkt) {
                return extract_and_update(codecpar, pkt, "hevc_mp4toannexb\0");
            }
        }
        // MPEG4
        AVCodecID_AV_CODEC_ID_MPEG4 => {
            return apply_bsf(st, "mpeg4_unpack_bframes\0");
        }
        // AAC
        AVCodecID_AV_CODEC_ID_AAC => {
            return apply_bsf(st, "aac_adtstoasc\0");
        }
        // PCM / G.7xx 系列（不需要 extradata）
        AVCodecID_AV_CODEC_ID_PCM_ALAW
        | AVCodecID_AV_CODEC_ID_PCM_MULAW
        | AVCodecID_AV_CODEC_ID_ADPCM_G722
        | AVCodecID_AV_CODEC_ID_G723_1
        | AVCodecID_AV_CODEC_ID_G729 => {
            return 0;
        }
        _ => {
            base::log::warn!("fix_extradata: no handler for codec_id={}", (*codecpar).codec_id);
            return -1;
        }
    }

    0
}

/// 判断是否为 IDR 关键帧（H.264/H.265）
/// 简单检查 flags 和 NAL 单元
unsafe fn is_idr(pkt: *mut AVPacket) -> bool {
    if (*pkt).flags & AV_PKT_FLAG_KEY as i32 != 0 {
        return true;
    }
    // 粗略 NAL 检查（前4字节 -> NAL type）
    let data = std::slice::from_raw_parts((*pkt).data, (*pkt).size as usize);
    if data.len() > 4 {
        let nal_type = data[4] & 0x1F;
        if nal_type == 5 { // H.264 IDR
            return true;
        }
        let nal_type_h265 = (data[4] >> 1) & 0x3F;
        if nal_type_h265 == 19 || nal_type_h265 == 20 { // H.265 IDR
            return true;
        }
    }
    false
}

/// 提取并更新 extradata
unsafe fn extract_and_update(codecpar: *mut AVCodecParameters, pkt: *mut AVPacket, bsf_name: &str) -> i32 {
    let mut bsf: *mut AVBSFContext = std::ptr::null_mut();
    let filter = av_bsf_get_by_name(bsf_name.as_ptr() as *const i8);
    if filter.is_null() {
        base::log::error!("BSF {} not found", bsf_name);
        return -1;
    }

    if av_bsf_alloc(filter, &mut bsf) < 0 {
        return -1;
    }

    avcodec_parameters_copy((*bsf).par_in, codecpar);
    if av_bsf_init(bsf) < 0 {
        av_bsf_free(&mut bsf);
        return -1;
    }

    // 把当前 pkt 喂入，尝试生成 extradata
    if av_bsf_send_packet(bsf, pkt) >= 0 {
        let mut out_pkt: *mut AVPacket = av_packet_alloc();
        while av_bsf_receive_packet(bsf, out_pkt) == 0 {
            av_packet_unref(out_pkt);
        }
        av_packet_free(&mut out_pkt);
    }

    // 更新 extradata
    avcodec_parameters_copy(codecpar, (*bsf).par_out);

    base::log::info!(
        "Updated extradata via {}: size={}",
        CStr::from_ptr((*filter).name).to_string_lossy(),
        (*codecpar).extradata_size
    );

    av_bsf_free(&mut bsf);
    0
}

/// 通用 bsf 调用（非逐包）
unsafe fn apply_bsf(st: *mut AVStream, bsf_name: &str) -> i32 {
    let mut bsf: *mut AVBSFContext = std::ptr::null_mut();
    let filter = av_bsf_get_by_name(bsf_name.as_ptr() as *const i8);
    if filter.is_null() {
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

    avcodec_parameters_copy((*st).codecpar, (*bsf).par_out);

    base::log::info!(
        "Applied bsf {} to stream, extradata size={}",
        bsf_name,
        (*(*st).codecpar).extradata_size
    );

    av_bsf_free(&mut bsf);
    0
}
