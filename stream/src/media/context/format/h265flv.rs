use crate::media::context::format::demuxer::{DemuxerContext, H265ParameterSets};
use crate::media::context::format::{FmtMuxer, MuxPacket};
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::tokio::sync::broadcast::Sender;
use log::{info, warn};
use rsmpeg::ffi::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

// FLV 标签类型常量
const FLV_TAG_AUDIO: u8 = 8;
const FLV_TAG_VIDEO: u8 = 9;

// FLV 编码类型
const FLV_CODEC_H265: u8 = 12;
const FLV_CODEC_AAC: u8 = 10;

// FLV 视频帧类型
const FLV_FRAME_KEY: u8 = 1;
const FLV_FRAME_INTER: u8 = 2;

// H265 NAL 类型
const H265_NAL_VPS: u8 = 32;
const H265_NAL_SPS: u8 = 33;
const H265_NAL_PPS: u8 = 34;
const H265_NAL_IDR_W_RADL: u8 = 19;
const H265_NAL_IDR_N_LP: u8 = 20;
const H265_NAL_CRA_NUT: u8 = 21;

// FLV 音频参数
const FLV_AUDIO_SAMPLERATE_44K: u8 = 3;
const FLV_AUDIO_SAMPLERATE_22K: u8 = 2;
const FLV_AUDIO_SAMPLERATE_11K: u8 = 1;
const FLV_AUDIO_SAMPLERATE_5_5K: u8 = 0;
const FLV_AUDIO_BITS_16: u8 = 1;
const FLV_AUDIO_BITS_8: u8 = 0;
const FLV_AUDIO_STEREO: u8 = 1;
const FLV_AUDIO_MONO: u8 = 0;

// G711 编码类型
const FLV_CODEC_G711_A: u8 = 7;
const FLV_CODEC_G711_MU: u8 = 8;

// FLV 时间基（毫秒）
const FLV_TIME_BASE: AVRational = AVRational { num: 1, den: 1000 };

#[derive(Clone, Debug)]
pub struct AudioStreamInfo {
    pub stream_index: i32,
    pub codec_id: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub extradata: Vec<u8>,
    pub time_base: AVRational,
}

impl Default for AudioStreamInfo {
    fn default() -> Self {
        Self {
            stream_index: -1,
            codec_id: 0,
            sample_rate: 0,
            channels: 0,
            extradata: Vec::new(),
            time_base: AVRational { num: 0, den: 0 },
        }
    }
}

pub struct H265FlvContext {
    pub tx: Sender<Arc<MuxPacket>>,

    // H265 参数集（用于关键帧检测和过滤）
    vps: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,

    // 直接使用 FFmpeg 提供的 extradata (HVCC)
    video_extradata: Vec<u8>,

    // 流索引和时间基
    video_stream_index: i32,
    video_time_base: AVRational,
    audio_streams: HashMap<i32, AudioStreamInfo>,

    // FLV 头部数据（包含文件头 + 所有序列头）
    header: Bytes,
    epoch: Instant,
}

impl H265FlvContext {

    fn ensure_hvcc(extradata: &[u8], vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
        if Self::is_hvcc(extradata) {
            extradata.to_vec()
        } else {
            Self::build_hvcc_from_nalus(vps, sps, pps)
        }
    }
    fn is_hvcc(data: &[u8]) -> bool {
        if data.len() < 23 {  // HVCC 至少需要 23 字节
            return false;
        }
        // 检查 configurationVersion 为 1
        data[0] == 1 &&
            // 检查 lengthSizeMinusOne 有效（通常为 3）
            (data[21] & 0x03) == 3
    }
    fn build_hvcc_from_nalus(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let mut hvcc = Vec::with_capacity(128);

        // ===== HEVCDecoderConfigurationRecord =====

        // configurationVersion
        hvcc.push(1);

        // general_profile_space(2) + tier_flag(1) + profile_idc(5)
        hvcc.push(0x01); // baseline，实际可从 SPS 解析（这里简化）

        // general_profile_compatibility_flags (4 bytes)
        hvcc.extend_from_slice(&[0, 0, 0, 0]);

        // general_constraint_indicator_flags (6 bytes)
        hvcc.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        // general_level_idc
        hvcc.push(120); // 默认 level（可从 SPS 解析，这里简化）

        // reserved (4 bits) + min_spatial_segmentation_idc (12 bits)
        hvcc.extend_from_slice(&[0xF0, 0x00]);

        // reserved (6) + parallelismType (2)
        hvcc.push(0xFC);

        // reserved (6) + chromaFormat (2)
        hvcc.push(0xFC | 1); // 4:2:0

        // reserved (5) + bitDepthLumaMinus8 (3)
        hvcc.push(0xF8);

        // reserved (5) + bitDepthChromaMinus8 (3)
        hvcc.push(0xF8);

        // avgFrameRate
        hvcc.extend_from_slice(&[0x00, 0x00]);

        // constantFrameRate(2) + numTemporalLayers(3) + temporalIdNested(1) + lengthSizeMinusOne(2)
        hvcc.push(0x03); // lengthSizeMinusOne = 3 → 4字节长度前缀

        // ===== NALU arrays =====

        let mut num_arrays = 0;
        if !vps.is_empty() { num_arrays += 1; }
        if !sps.is_empty() { num_arrays += 1; }
        if !pps.is_empty() { num_arrays += 1; }

        hvcc.push(num_arrays);

        // helper
        fn write_array(buf: &mut Vec<u8>, nal_type: u8, nal: &[u8]) {
            if nal.is_empty() {
                return;
            }

            // array_completeness(1) + reserved(1) + nal_unit_type(6)
            buf.push(0x80 | nal_type);

            // numNalus
            buf.extend_from_slice(&1u16.to_be_bytes());

            // nalUnitLength
            buf.extend_from_slice(&(nal.len() as u16).to_be_bytes());

            // nalUnit
            buf.extend_from_slice(nal);
        }

        write_array(&mut hvcc, H265_NAL_VPS, vps);
        write_array(&mut hvcc, H265_NAL_SPS, sps);
        write_array(&mut hvcc, H265_NAL_PPS, pps);

        hvcc
    }
    /// 判断是否为关键帧
    fn is_keyframe(nal_type: u8) -> bool {
        matches!(
            nal_type,
            H265_NAL_IDR_W_RADL | H265_NAL_IDR_N_LP | H265_NAL_CRA_NUT
        )
    }

    /// 从 NAL 单元提取类型
    fn get_nal_type(nalu: &[u8]) -> u8 {
        if nalu.is_empty() {
            return 0;
        }
        (nalu[0] >> 1) & 0x3F
    }

    /// Annex B 转 AVCC (长度前缀格式)
    fn annexb_to_avcc_fast<F>(data: &[u8], mut filter: F) -> BytesMut
    where
        F: FnMut(u8) -> bool, // true = 保留
    {
        let len = data.len();
        let mut out = BytesMut::with_capacity(len + 64); // 预留一点避免扩容

        let mut i = 0;
        let mut nal_start = 0;
        let mut found_start = false;

        while i + 3 < len {
            if data[i] == 0 && data[i + 1] == 0 {
                let sc_len = if data[i + 2] == 1 {
                    3
                } else if i + 3 < len && data[i + 2] == 0 && data[i + 3] == 1 {
                    4
                } else {
                    i += 1;
                    continue;
                };

                if found_start {
                    let nal = &data[nal_start..i];
                    if !nal.is_empty() {
                        let nal_type = (nal[0] >> 1) & 0x3F;

                        if filter(nal_type) {
                            out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                            out.extend_from_slice(nal);
                        }
                    }
                }

                nal_start = i + sc_len;
                found_start = true;
                i += sc_len;
            } else {
                i += 1;
            }
        }

        // flush last NAL
        if found_start && nal_start < len {
            let nal = &data[nal_start..len];
            if !nal.is_empty() {
                let nal_type = (nal[0] >> 1) & 0x3F;

                if filter(nal_type) {
                    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    out.extend_from_slice(nal);
                }
            }
        }

        out
    }

    /// 解析 AVCC 格式的 NAL 单元（带长度前缀）
    fn parse_avcc(data: &[u8]) -> Vec<&[u8]> {
        let mut nalus = Vec::new();
        let mut offset = 0;
        let len = data.len();

        while offset + 4 <= len {
            let nal_len = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;

            offset += 4;
            if offset + nal_len > len {
                break;
            }

            nalus.push(&data[offset..offset + nal_len]);
            offset += nal_len;
        }

        nalus
    }

    /// 写入 NAL 单元（带长度前缀）
    fn write_nalu(nal: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }

    /// 获取 FLV 音频采样率索引
    fn get_flv_samplerate_index(sample_rate: u32) -> u8 {
        match sample_rate {
            5512 => FLV_AUDIO_SAMPLERATE_5_5K,
            11025 => FLV_AUDIO_SAMPLERATE_11K,
            22050 => FLV_AUDIO_SAMPLERATE_22K,
            _ => FLV_AUDIO_SAMPLERATE_44K,
        }
    }

    /// 构建视频序列头（直接使用 FFmpeg 的 extradata）
    fn build_video_sequence_header(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(5 + self.video_extradata.len());

        body.push((FLV_FRAME_KEY << 4) | FLV_CODEC_H265);
        body.push(0); // packet type: sequence header
        body.extend_from_slice(&[0, 0, 0]); // CTS = 0
        body.extend_from_slice(&self.video_extradata);

        Self::build_flv_tag(FLV_TAG_VIDEO, body, 0)
    }

    /// 构建视频数据标签（不再包含 VPS/SPS/PPS）
    fn build_video_data_tag(
        &self,
        frame_type: u8,
        cts: i32,
        payload: Vec<u8>,
        dts: u32,
    ) -> Vec<u8> {
        let mut body = Vec::with_capacity(5 + payload.len());

        body.push((frame_type << 4) | FLV_CODEC_H265);
        body.push(1); // packet type: NAL unit
        body.extend_from_slice(&cts.to_be_bytes()[1..]); // CTS (3 bytes)
        body.extend_from_slice(&payload);

        Self::build_flv_tag(FLV_TAG_VIDEO, body, dts)
    }

    /// 构建音频序列头
    fn build_audio_sequence_header(info: &AudioStreamInfo) -> GlobalResult<Vec<u8>> {
        let mut body = Vec::with_capacity(2 + info.extradata.len());

        // 根据编码类型构建音频头
        let audio_header = match info.codec_id as u32 {
            AVCodecID_AV_CODEC_ID_AAC => {
                (FLV_CODEC_AAC << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            AVCodecID_AV_CODEC_ID_PCM_ALAW => {
                (FLV_CODEC_G711_A << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            AVCodecID_AV_CODEC_ID_PCM_MULAW => {
                (FLV_CODEC_G711_MU << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            _ => {
                return Err(GlobalError::new_sys_error(
                    &format!("Unsupported audio codec: {}", info.codec_id),
                    |msg| warn!("{}", msg),
                ));
            }
        };

        body.push(audio_header);

        // AAC 需要发送配置信息
        if info.codec_id == AVCodecID_AV_CODEC_ID_AAC {
            body.push(0); // packet type: config
            body.extend_from_slice(&info.extradata);
        }

        Ok(Self::build_flv_tag(FLV_TAG_AUDIO, body, 0))
    }

    /// 构建音频数据标签
    fn build_audio_data_tag(
        &self,
        info: &AudioStreamInfo,
        raw_audio: &[u8],
        dts: u32,
    ) -> GlobalResult<Vec<u8>> {
        let mut body = Vec::with_capacity(2 + raw_audio.len());

        // 根据编码类型构建音频头
        let audio_header = match info.codec_id {
            AVCodecID_AV_CODEC_ID_AAC => {
                (FLV_CODEC_AAC << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            AVCodecID_AV_CODEC_ID_PCM_ALAW => {
                (FLV_CODEC_G711_A << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            AVCodecID_AV_CODEC_ID_PCM_MULAW => {
                (FLV_CODEC_G711_MU << 4)
                    | (Self::get_flv_samplerate_index(info.sample_rate) << 2)
                    | (FLV_AUDIO_BITS_16 << 1)
                    | (if info.channels == 2 {
                        FLV_AUDIO_STEREO
                    } else {
                        FLV_AUDIO_MONO
                    })
            }
            _ => {
                return Err(GlobalError::new_sys_error(
                    &format!("Unsupported audio codec: {}", info.codec_id),
                    |msg| warn!("{}", msg),
                ));
            }
        };

        body.push(audio_header);

        // AAC 需要标记为 raw packet
        if info.codec_id == AVCodecID_AV_CODEC_ID_AAC {
            body.push(1); // packet type: raw
        }

        body.extend_from_slice(raw_audio);

        Ok(Self::build_flv_tag(FLV_TAG_AUDIO, body, dts))
    }

    /// 构建通用 FLV 标签
    fn build_flv_tag(tag_type: u8, body: Vec<u8>, dts: u32) -> Vec<u8> {
        let mut tag = Vec::with_capacity(11 + body.len() + 4);

        // Tag header
        tag.push(tag_type);
        tag.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]); // 3 bytes
        tag.extend_from_slice(&dts.to_be_bytes()[1..]); // 3 bytes
        tag.push((dts >> 24) as u8); // timestamp high
        tag.extend_from_slice(&[0, 0, 0]); // StreamID

        // Tag body
        tag.extend_from_slice(&body);

        // Previous tag size
        tag.extend_from_slice(&(tag.len() as u32).to_be_bytes());

        tag
    }

    /// 移除 AAC ADTS 头
    fn strip_adts(data: &[u8]) -> &[u8] {
        if data.len() >= 7 && data[0] == 0xFF && (data[1] & 0xF0) == 0xF0 {
            let has_crc = (data[1] & 0x01) == 0;
            let header_len = if has_crc { 9 } else { 7 };
            if data.len() > header_len {
                return &data[header_len..];
            }
        }
        data
    }

    /// 时间戳转换：从源时间基转换到 FLV 时间基（毫秒）
    fn rescale_ts(ts: i64, src_tb: AVRational, dst_tb: AVRational) -> i64 {
        unsafe { av_rescale_q(ts, src_tb, dst_tb) }
    }

    /// 发送 MuxPacket
    fn send_packet(
        tx: &Sender<Arc<MuxPacket>>,
        epoch: Instant,
        data: Vec<u8>,
        timestamp: u64,
        is_key: bool,
    ) -> GlobalResult<()> {
        let mux_packet = MuxPacket {
            data: Bytes::from(data),
            is_key,
            timestamp,
            epoch,
            seq: 0,
        };

        tx.send(Arc::new(mux_packet)).map_err(|e| {
            GlobalError::new_sys_error(&format!("Failed to send packet: {}", e), |msg| {
                warn!("{}", msg)
            })
        })?;
        Ok(())
    }

    /// 构建完整的 FLV header（包含文件头 + 所有序列头）
    fn build_full_header(&mut self) -> GlobalResult<Bytes> {
        let mut header = vec![
            0x46, 0x4C, 0x56, // "FLV"
            0x01, // version 1
            0x05, // flags: audio + video
            0x00, 0x00, 0x00, 0x09, // data offset
            0x00, 0x00, 0x00, 0x00, // previous tag size 0
        ];

        // 添加视频序列头（包含 VPS/SPS/PPS）
        let video_seq_header = self.build_video_sequence_header();
        header.extend_from_slice(&video_seq_header);
        info!(
            "Added video sequence header ({} bytes)",
            video_seq_header.len()
        );

        // 添加所有音频序列头
        for (stream_idx, info) in self.audio_streams.iter() {
            if !info.extradata.is_empty() && info.codec_id == AVCodecID_AV_CODEC_ID_AAC {
                let audio_seq_header = Self::build_audio_sequence_header(info)?;
                header.extend_from_slice(&audio_seq_header);
                info!(
                    "Added AAC sequence header for stream {} ({} bytes)",
                    stream_idx,
                    audio_seq_header.len()
                );
            }
        }

        info!(
            "Built full FLV header with total size: {} bytes",
            header.len()
        );
        Ok(Bytes::from(header))
    }
}

impl FmtMuxer for H265FlvContext {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self> {
        let mut ctx = H265FlvContext {
            tx: pkt_tx,
            vps: vec![],
            sps: vec![],
            pps: vec![],
            video_extradata: Default::default(),
            video_stream_index: -1,
            video_time_base: AVRational { num: 0, den: 0 },
            audio_streams: HashMap::new(),
            header: Bytes::new(),
            epoch: Instant::now(),
        };

        unsafe {
            let fmt_ctx = demuxer_context.avio.fmt_ctx;
            let nb = (*fmt_ctx).nb_streams as usize;

            for i in 0..nb {
                let stream = *(*fmt_ctx).streams.add(i);
                let codecpar = (*stream).codecpar;
                let time_base = (*stream).time_base;

                match (*codecpar).codec_type {
                    AVMediaType_AVMEDIA_TYPE_VIDEO => {
                        // 保存 VPS/SPS/PPS 原始数据（用于关键帧检测和过滤）
                        if let Some(Some(param)) = demuxer_context.params.get(i).map(|p| &p.h265_ps)
                        {
                            if let H265ParameterSets {
                                vps: Some(vps),
                                sps: Some(sps),
                                pps: Some(pps),
                            } = param
                            {
                                ctx.vps.extend_from_slice(vps);
                                ctx.sps.extend_from_slice(sps);
                                ctx.pps.extend_from_slice(pps);
                                ctx.video_stream_index = i as i32;
                                ctx.video_time_base = time_base;
                                // 保存 FFmpeg 提供的 extradata (HVCC)
                                if (*codecpar).extradata_size > 0 {
                                    let video_extradata = std::slice::from_raw_parts(
                                        (*codecpar).extradata,
                                        (*codecpar).extradata_size as usize,
                                    );
                                    ctx.video_extradata = Self::ensure_hvcc(
                                        video_extradata,
                                        &ctx.vps,
                                        &ctx.sps,
                                        &ctx.pps,
                                    );
                                    info!("Video extradata size: {} bytes", ctx.video_extradata.len());
                                } else {
                                    return Err(GlobalError::new_sys_error(
                                        "Video stream missing extradata",
                                        |msg| warn!("{}", msg),
                                    ));
                                }


                                info!(
                                    "Found H265 video stream {} with VPS/SPS/PPS, time_base: {}/{}",
                                    i, time_base.num, time_base.den
                                );
                            } else {
                                return Err(GlobalError::new_sys_error(
                                    "Missing VPS/SPS/PPS in H265 stream",
                                    |msg| warn!("{}", msg),
                                ));
                            }
                        } else {
                            return Err(GlobalError::new_sys_error(
                                "Missing H265ParameterSets",
                                |msg| warn!("{}", msg),
                            ));
                        }
                    }
                    AVMediaType_AVMEDIA_TYPE_AUDIO => {
                        let extradata = if (*codecpar).extradata_size > 0 {
                            std::slice::from_raw_parts(
                                (*codecpar).extradata,
                                (*codecpar).extradata_size as usize,
                            )
                            .to_vec()
                        } else {
                            Vec::new()
                        };

                        let audio_info = AudioStreamInfo {
                            stream_index: i as i32,
                            codec_id: (*codecpar).codec_id,
                            sample_rate: (*codecpar).sample_rate as u32,
                            channels: (*codecpar).ch_layout.nb_channels as u32,
                            extradata,
                            time_base,
                        };

                        ctx.audio_streams.insert(i as i32, audio_info);
                        info!(
                            "Found audio stream {} codec_id: {}, time_base: {}/{}",
                            i,
                            (*codecpar).codec_id,
                            time_base.num,
                            time_base.den
                        );
                    }
                    _ => {}
                }
            }
        }

        if ctx.video_stream_index == -1 {
            return Err(GlobalError::new_sys_error("No video stream found", |msg| {
                warn!("{}", msg)
            }));
        }

        if ctx.video_extradata.is_empty() {
            return Err(GlobalError::new_sys_error(
                "Video extradata is empty",
                |msg| warn!("{}", msg),
            ));
        }

        // 构建完整的 FLV header（文件头 + 所有序列头）
        ctx.header = ctx.build_full_header()?;

        info!(
            "H265FlvContext initialized successfully, header size: {} bytes",
            ctx.header.len()
        );
        Ok(ctx)
    }

    fn get_header(&self) -> Bytes {
        // 返回完整的 FLV header
        self.header.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()> {
        if pkt.stream_index == self.video_stream_index {
            // 处理视频包
            unsafe {
                if pkt.data.is_null() || pkt.size <= 0 {
                    return Ok(());
                }

                let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);
                if data.is_empty() {
                    return Ok(());
                }

                // 时间戳转换
                let dts_ms = Self::rescale_ts(pkt.dts, self.video_time_base, FLV_TIME_BASE);
                let pts_ms = Self::rescale_ts(pkt.pts, self.video_time_base, FLV_TIME_BASE);
                let dts = dts_ms.max(0);
                let pts = pts_ms.max(0);
                let mut is_key = false;
                // Annex B 转 AVCC
                let avcc = Self::annexb_to_avcc_fast(data, |nal_type| {
                    // 判断关键帧
                    if Self::is_keyframe(nal_type) {
                        is_key = true;
                    }

                    // 过滤掉不需要的 NAL
                    !matches!(
                        nal_type,
                        H265_NAL_VPS | H265_NAL_SPS | H265_NAL_PPS | 35 | 39 | 40 | 41 | 42 // AUD / SEI
                    )
                });
                if avcc.is_empty() {
                    return Ok(());
                }

                let cts = if pts > dts { (pts - dts) as i32 } else { 0 };
                let frame_type = if is_key {
                    FLV_FRAME_KEY
                } else {
                    FLV_FRAME_INTER
                };

                let tag = self.build_video_data_tag(frame_type, cts, avcc.to_vec(), dts as u32);
                Self::send_packet(&self.tx, self.epoch, tag, timestamp, is_key)?;
            }
        } else if let Some(audio_info) = self.audio_streams.get(&pkt.stream_index) {
            // 处理音频包
            unsafe {
                if pkt.data.is_null() || pkt.size <= 0 {
                    return Ok(());
                }

                let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize);
                if data.is_empty() {
                    return Ok(());
                }

                let dts_ms = Self::rescale_ts(pkt.dts, audio_info.time_base, FLV_TIME_BASE);
                let dts = dts_ms.max(0) as u32;

                let raw_audio = if audio_info.codec_id == AVCodecID_AV_CODEC_ID_AAC {
                    Self::strip_adts(data).to_vec()
                } else {
                    data.to_vec()
                };

                let tag = self.build_audio_data_tag(audio_info, &raw_audio, dts)?;
                Self::send_packet(&self.tx, self.epoch, tag, timestamp, false)?;
            }
        }

        Ok(())
    }

    fn flush(&mut self) {
        info!("Flushing H265FlvContext");
    }
}
