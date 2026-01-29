use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::FmtMuxer;
use crate::media::context::format::demuxer::{DemuxerContext, ParamStream};
use crate::media::context::format::muxer::MuxerContext;
use crate::media::rtp::RtpPacketBuffer;
use crate::state::layer::muxer_layer::MuxerLayer;
use crate::state::msg::StreamConfig;
use base::bus::mpsc::TypedReceiver;
use base::exception::GlobalResult;
use base::exception::typed::common::MessageBusError;
use base::log::{debug, error};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use rsmpeg::ffi::{AVPacket, av_free, av_malloc};
use shared::info::media_info_ext::MediaExt;
use std::ptr;
use std::sync::Arc;
use log::info;

mod codec;
pub mod event;
mod filter;
pub mod format;
pub mod utils;

/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步

pub struct RtpState {
    pub timestamp: u32, // 读取rtp包的timestamp
    pub marker: bool,   // 读取rtp包的mark

    pub last_32: u32,        // 上一次 RTP timestamp（32-bit）
    pub last_unwrapped: i64, // 上一次展开 timestamp，用于累积 diff
}
impl RtpState {
    pub fn new() -> Self {
        Self {
            timestamp: 0,
            marker: false,
            last_32: 0,
            last_unwrapped: 0,
        }
    }

    /// 更新 RTP 状态，返回当前展开 timestamp 和帧间差值
    /// `clock_rate` 用于最大 diff 限制
    pub fn update(&mut self, cur_ts: u32, clock_rate: u32) -> (i64, i64) {
        let cur_unwrapped = if self.last_unwrapped == 0 {
            // 第一帧
            cur_ts as i64
        } else {
            let mut diff = (cur_ts as i64).wrapping_sub(self.last_32 as i64);

            // wrap-around 检测
            if diff < 0 && (self.last_32.wrapping_sub(cur_ts) > 0x8000_0000) {
                diff = (cur_ts as i64 + (1i64 << 32)) - self.last_32 as i64;
            }

            // 最大 diff 限制，防止异常跳变
            let max_diff = clock_rate as i64; // 1 秒最大 diff，可按需要调整
            if diff < 0 {
                diff = 0;
            } else if diff > max_diff {
                diff = max_diff;
            }

            self.last_unwrapped + diff
        };

        let duration_ticks = if self.last_unwrapped == 0 {
            0
        } else {
            cur_unwrapped - self.last_unwrapped
        };

        // 更新状态
        self.last_unwrapped = cur_unwrapped;
        self.last_32 = cur_ts;

        (cur_unwrapped, duration_ticks)
    }
}
pub struct MediaContext {
    pub ssrc: u32,
    pub media_ext: MediaExt,
    pub codec_context: Option<CodecContext>,
    pub filter_context: FilterContext,
    pub muxer_context: MuxerContext,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub demuxer_context: DemuxerContext,
    pub rtp_state: *mut RtpState,
    /// 是否还允许修复 codecpar
    pub codecpar_fixable: bool,
}
impl Drop for MediaContext {
    fn drop(&mut self) {
        unsafe {
            if !self.rtp_state.is_null() {
                // 回收 RtpState
                drop(Box::from_raw(self.rtp_state));
                self.rtp_state = std::ptr::null_mut();
            }
        }
    }
}

impl MediaContext {
    pub fn init(
        ssrc: u32,
        stream_config: StreamConfig,
    ) -> GlobalResult<(MediaContext, MuxerLayer)> {
        let rtp_buffer = RtpPacketBuffer::init(ssrc, stream_config.rtp_rx)?;
        // Box → raw pointer
        let rtp_state_ptr = Box::into_raw(Box::new(RtpState::new()));
        let demuxer_context = DemuxerContext::start_demuxer(
            ssrc,
            &stream_config.media_ext,
            rtp_buffer,
            rtp_state_ptr,
        )?;
        let converter = stream_config.converter;

        let context = MediaContext {
            codec_context: CodecContext::init(converter.codec),
            filter_context: FilterContext::init(converter.filter),
            ssrc,
            media_ext: stream_config.media_ext,
            context_event_rx: stream_config.context_event_rx,
            muxer_context: Default::default(),
            demuxer_context,
            rtp_state: rtp_state_ptr,
            codecpar_fixable: true,
        };
        Ok((context, converter.muxer))
    }

    pub fn invoke(&mut self, muxer_layer: MuxerLayer) {
        use rsmpeg::ffi::{AVRational, av_rescale_q};

        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            //write start
            let mut retry = 0;
            //write body
            let mut pkt = std::mem::zeroed::<AVPacket>();
            loop {
                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if ret < 0 {
                    return;
                }
                if self.codecpar_fixable {
                    if repair_codecpar_extradata(&pkt, &mut self.demuxer_context.params) {
                        self.codecpar_fixable = false;
                        self.muxer_context =
                            MuxerContext::init(&self.demuxer_context, &muxer_layer);
                    } else {
                        retry += 1;
                        rsmpeg::ffi::av_packet_unref(&mut pkt);
                        if retry < 240 {
                            continue;
                        } else {
                            error!("ssrc={}: repair codecpar extradata failed", self.ssrc);
                            return;
                        }
                    }
                }
                match self.context_event_rx.try_recv() {
                    Ok(event) => self.handle_event(event),
                    Err(MessageBusError::ChannelClosed) => break,
                    Err(_) => {}
                }
                //fill_stream_from_media_ext(st, media_ext);
                let tb = (*(*fmt_ctx).streams.offset(pkt.stream_index as isize).read()).time_base;

                // 更新 RTP 状态并获取展开 timestamp 和帧间差值
                let rtp_state = &mut *self.rtp_state;
                let (cur_unwrapped, duration_ticks) =
                    rtp_state.update(rtp_state.timestamp, self.media_ext.clock_rate as u32);

                let rtp_tb = AVRational {
                    num: 1,
                    den: self.media_ext.clock_rate,
                };
                let pts_rescaled = av_rescale_q(cur_unwrapped, rtp_tb, tb);
                let duration_rescaled = if duration_ticks > 0 {
                    av_rescale_q(duration_ticks, rtp_tb, tb)
                } else {
                    av_rescale_q((self.media_ext.clock_rate / 25) as i64, rtp_tb, tb)
                };
                pkt.duration = duration_rescaled;

                // info!(
                //     "DEMX RTP: raw_ts={} unwrapped={} pts={} dts={} duration={} (tb={}/{})",
                //     rtp_state.last_32,
                //     cur_unwrapped,
                //     pkt.pts,
                //     pkt.dts,
                //     pkt.duration,
                //     tb.num,
                //     tb.den
                // );

                // 通过 pts 计算累计真实时长（秒）
                let real_ts = pts_rescaled as f64 * tb.num as f64 / tb.den as f64;

                // 暂不实现处理codec
                // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
                // 暂不实现处理filter
                // Self::handle_filter(&mut self.filter_context);

                // 调用 muxer
                Self::handle_pkt_muxer(&mut self.muxer_context, &pkt, real_ts as u64);

                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
            //write end
            Self::handle_pkt_muxer_end(&mut self.muxer_context);
        }

        fn rpt_diff_u32(a: u32, b: u32) -> u32 {
            if a >= b { a - b } else { b.wrapping_sub(a) }
        }
    }

    fn handle_codec(codec: &mut CodecContext) {}
    fn handle_filter(filter: &mut FilterContext) {}

    // 1.写入头信息
    // 2.循环写入body
    // 3.写入结束信息
    // 问题如何传递信息【该使用写入结束信息】
    // 回调
    fn handle_pkt_muxer(muxer: &mut MuxerContext, pkt: &AVPacket, ts: u64) {
        if let Some(context) = &mut muxer.flv {
            context.write_packet(pkt, ts);
        }
        if let Some(context) = &mut muxer.mp4 {
            context.write_packet(pkt, ts);
        }
        if let Some(context) = &muxer.ts {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_frame {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_ps {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_enc {
            unimplemented!()
        }
        if let Some(context) = &muxer.hls_ts {
            unimplemented!()
        }
        if let Some(context) = &mut muxer.fmp4 {
            context.write_packet(pkt, ts);
        }
    }
    fn handle_pkt_muxer_end(muxer: &mut MuxerContext) {
        if let Some(context) = &mut muxer.flv {
            context.flush();
        }
        if let Some(context) = &mut muxer.mp4 {
            context.flush();
        }
        if let Some(context) = &muxer.ts {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_frame {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_ps {
            unimplemented!()
        }
        if let Some(context) = &muxer.rtp_enc {
            unimplemented!()
        }
        if let Some(context) = &muxer.hls_ts {
            unimplemented!()
        }
        if let Some(context) = &mut muxer.fmp4 {
            context.flush();
        }
    }

    fn handle_event(&mut self, event: ContextEvent) {
        match event {
            ContextEvent::Codec(_) => {
                unimplemented!()
            }
            ContextEvent::Muxer(m_event) => {
                m_event.handle_event(&mut self.muxer_context, &self.demuxer_context);
            }
            ContextEvent::Filter(_) => {
                unimplemented!()
            }
            ContextEvent::Inner(i_event) => {
                i_event.handle_event(&self);
            }
        }
    }
}
#[derive(Default)]
pub struct H264ParameterSets {
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct H265ParameterSets {
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
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
unsafe fn repair_codecpar_extradata(
    pkt: &AVPacket,
    demuxer_streams: &mut Vec<ParamStream>,
) -> bool {
    let mut all_ready = true;

    for param_stream in demuxer_streams.iter_mut() {
        let codecpar = param_stream.codecpar;
        debug!(
    "codec_id(enum)={} codec_tag={} extradata_size={}",
    (*codecpar).codec_id,
    (*codecpar).codec_tag,
    (*codecpar).extradata_size
);
        match (*codecpar).codec_id {
            AVCodecID_AV_CODEC_ID_H264 => {
                // 打印当前extradata状态
                if !(*codecpar).extradata.is_null() && (*codecpar).extradata_size > 0 {
                    let size = (*codecpar).extradata_size as usize;
                    let slice = std::slice::from_raw_parts((*codecpar).extradata, size.min(32));
                    debug!("Current H264 extradata (first {} of {}): {:02X?}",
                        slice.len(), size, slice);
                }

                // 修复 H264 PS
                let ps = param_stream
                    .repair
                    .h264_ps
                    .get_or_insert_with(Default::default);
                extract_h264_ps(pkt, ps);

                if ps.sps.is_none() || ps.pps.is_none() {
                    all_ready = false;
                    debug!("H264: Waiting for SPS/PPS");
                    continue;
                }

                let sps = ps.sps.as_ref().unwrap();
                let pps = ps.pps.as_ref().unwrap();

                debug!("H264 SPS ({} bytes): {:02X?}", sps.len(), sps);
                debug!("H264 PPS ({} bytes): {:02X?}", pps.len(), pps);

                let extradata_size = 4 + sps.len() + 4 + pps.len();
                let extradata = av_malloc(extradata_size) as *mut u8;

                // 验证内存分配
                if extradata.is_null() {
                    error!("Failed to allocate {} bytes for extradata", extradata_size);
                    all_ready = false;
                    continue;
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
                    error!("Extradata size mismatch: expected {}, got {}", extradata_size, offset);
                    av_free(extradata as *mut _);
                    all_ready = false;
                    continue;
                }

                // 打印新建的extradata
                let new_extradata_slice = std::slice::from_raw_parts(extradata, extradata_size);
                debug!("New H264 AnnexB extradata ({} bytes): {:02X?}",
                    extradata_size, new_extradata_slice);

                // 释放旧 extradata
                if !(*codecpar).extradata.is_null() {
                    debug!("Freeing old extradata at {:p}", (*codecpar).extradata);
                    av_free((*codecpar).extradata as *mut _);
                }

                (*codecpar).extradata = extradata;
                (*codecpar).extradata_size = extradata_size as i32;

                debug!("H264 extradata updated: ptr={:p}, size={}",
                    (*codecpar).extradata, (*codecpar).extradata_size);
            }

            AVCodecID_AV_CODEC_ID_HEVC => {
                // 修复 H265 PS
                let ps = param_stream
                    .repair
                    .h265_ps
                    .get_or_insert_with(Default::default);
                extract_h265_ps(pkt, ps);

                if ps.vps.is_none() || ps.sps.is_none() || ps.pps.is_none() {
                    all_ready = false;
                    continue;
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

                if !(*codecpar).extradata.is_null() {
                    av_free((*codecpar).extradata as *mut _);
                }

                (*codecpar).extradata = extradata;
                (*codecpar).extradata_size = extradata_size as i32;
            }

            AVCodecID_AV_CODEC_ID_AAC => {
                // 修复 AAC ASC
                if param_stream.repair.aac_asc.is_none() {
                    if let Some(asc) = extract_aac_asc(pkt) {
                        param_stream.repair.aac_asc = Some(asc);

                        if !(*codecpar).extradata.is_null() {
                            av_free((*codecpar).extradata as *mut _);
                        }

                        (*codecpar).extradata = av_malloc(2) as *mut u8;
                        (*codecpar).extradata_size = 2;
                        ptr::copy_nonoverlapping(asc.as_ptr(), (*codecpar).extradata, 2);
                    } else {
                        all_ready = false;
                    }
                }
            }

            _ => {}
        }
        debug!("codecpar extradata_size={}", (*codecpar).extradata_size);
    }

    all_ready
}
