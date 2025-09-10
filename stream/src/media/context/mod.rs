use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::muxer::MuxerContext;
use crate::media::rtp::RtpPacketBuffer;
use crate::state::msg::StreamConfig;
use base::bus::mpsc::TypedReceiver;
use base::exception::typed::common::MessageBusError;
use base::exception::GlobalResult;
use rsmpeg::ffi::AVPacket;
use base::log::{debug, warn};
use shared::info::media_info_ext::MediaExt;
use std::time::Instant;

pub mod event;
pub mod format;
mod codec;
mod filter;
mod utils;

/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步


pub struct RtpState {
    /// 原来的字段（保留）
    pub timestamp: u32,
    pub marker: bool,

    /// unwrap 状态
    pub wraps: u64,            // 已经经历的 32-bit wrap 次数
    pub last_32: u32,          // 上次记录的低 32 位（用于判断 wrap）
    pub last_unwrapped: i64,   // 上次完整展开后的 timestamp（单位：RTP ticks，90kHz）
    pub last_pts: i64,         // 上次转换后的 pts（单位：目标 tb）
    pub initialized: bool,     // 是否已初始化
    pub last_arrival: Instant, // 上次包到达时间（用于估算 expected increment）
}

impl RtpState {
    pub fn new() -> Self {
        Self {
            timestamp: 0,
            marker: false,
            wraps: 0,
            last_32: 0,
            last_unwrapped: 0,
            last_pts: 0,
            initialized: false,
            last_arrival: Instant::now(),
        }
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
    pub fn init(ssrc: u32, stream_config: StreamConfig) -> GlobalResult<MediaContext> {
        let rtp_buffer = RtpPacketBuffer::init(ssrc, stream_config.rtp_rx)?;
        // Box → raw pointer
        let rtp_state_ptr = Box::into_raw(Box::new(RtpState::new()));
        let demuxer_context = DemuxerContext::start_demuxer(ssrc, &stream_config.media_ext, rtp_buffer, rtp_state_ptr)?;
        let converter = stream_config.converter;
        let context = MediaContext {
            codec_context: CodecContext::init(converter.codec),
            filter_context: FilterContext::init(converter.filter),
            ssrc,
            media_ext: stream_config.media_ext,
            context_event_rx: stream_config.context_event_rx,
            muxer_context: MuxerContext::init(&demuxer_context, converter.muxer),
            demuxer_context,
            rtp_state: rtp_state_ptr,
        };
        Ok(context)
    }

    pub fn invoke(&mut self) {
        use rsmpeg::ffi::{AVRational, av_rescale_q};

        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            let mut pkt = std::mem::zeroed::<AVPacket>();
            loop {
                // 处理converter事件
                match self.context_event_rx.try_recv() {
                    Ok(event) => { self.handle_event(event) }
                    Err(MessageBusError::ChannelClosed) => { break; }
                    Err(_) => {}
                }

                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if ret < 0 {
                    break;
                }

                // --- 读取 stream 的 time_base（目标 tb） ---
                let tb = (*(*fmt_ctx)
                    .streams
                    .offset(pkt.stream_index as isize)
                    .read())
                    .time_base;

                // --- RTP timestamp 解包 ---
                let rtp_state = &mut *self.rtp_state;
                let cur_ts_32 = rtp_state.timestamp;

                // wrap-around 检测
                if rtp_state.initialized {
                    if cur_ts_32 < rtp_state.last_32
                        && (rpt_diff_u32(rtp_state.last_32, cur_ts_32) > 0x8000_0000u32)
                    {
                        rtp_state.wraps = rtp_state.wraps.wrapping_add(1);
                    }
                } else {
                    rtp_state.initialized = true;
                    rtp_state.wraps = 0;
                }

                // 展开成 64-bit
                let cur_unwrapped =
                    (rtp_state.wraps as u128 * 0x1_0000_0000u128) + (cur_ts_32 as u128);
                let cur_unwrapped_i64 = cur_unwrapped as i64;

                // --- 计算 duration：严格按 RTP ts 差值 ---
                let mut duration_90k: i64 = 0;
                if rtp_state.last_unwrapped > 0 {
                    let diff = cur_unwrapped_i64 - rtp_state.last_unwrapped;
                    if diff > 0 {
                        duration_90k = diff;
                    }
                }

                // --- 映射到流 time_base ---
                let rtp_tb = AVRational { num: 1, den: self.media_ext.clock_rate };
                let pts_rescaled = av_rescale_q(cur_unwrapped_i64, rtp_tb, tb);

                let duration_rescaled = if duration_90k > 0 {
                    av_rescale_q(duration_90k, rtp_tb, tb)
                } else {
                    // fallback：如果第一帧没有 diff，就估一个（比如 1 帧时间）
                    av_rescale_q((self.media_ext.clock_rate / 25) as i64, rtp_tb, tb)
                };

                // 更新 state
                rtp_state.last_unwrapped = cur_unwrapped_i64;
                rtp_state.last_32 = cur_ts_32;
                rtp_state.last_pts = pts_rescaled;

                // 写回 pkt
                pkt.pts = pts_rescaled;
                pkt.dts = pts_rescaled;
                pkt.duration = duration_rescaled;
                debug!(
                "DEMX RTP: raw_ts={} unwrapped={} diff_90k={} pts={} dts={} duration={} (tb={}/{})",
                cur_ts_32,
                cur_unwrapped_i64,
                duration_90k,
                pkt.pts,
                pkt.dts,
                pkt.duration,
                tb.num,
                tb.den
            );

                // 暂不实现处理codec
                // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
                // 暂不实现处理filter
                // Self::handle_filter(&mut self.filter_context);

                // 调用 muxer
                Self::handle_muxer(&mut self.muxer_context, &pkt);

                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
        }

        fn rpt_diff_u32(a: u32, b: u32) -> u32 {
            if a >= b { a - b } else { b.wrapping_sub(a) }
        }
    }



    fn handle_codec(codec: &mut CodecContext) {}
    fn handle_filter(filter: &mut FilterContext) {}

    fn handle_muxer(muxer: &mut MuxerContext, pkt: &AVPacket) {
        if let Some(flv_context) = &mut muxer.flv {
            flv_context.write_packet(pkt);
        }
        if let Some(mp4_context) = &muxer.mp4 { unimplemented!() }
        if let Some(ts_context) = &muxer.ts { unimplemented!() }
        if let Some(rtp_frame_context) = &muxer.rtp_frame { unimplemented!() }
        if let Some(rtp_ps_context) = &muxer.rtp_ps { unimplemented!() }
        if let Some(rtp_enc_context) = &muxer.rtp_enc { unimplemented!() }
        if let Some(frame_context) = &muxer.frame { unimplemented!() }
    }

    fn handle_event(&mut self, event: ContextEvent) {
        match event {
            ContextEvent::Codec(_) => { unimplemented!() }
            ContextEvent::Muxer(m_event) => {
                m_event.handle_event(&mut self.muxer_context, &self.demuxer_context);
            }
            ContextEvent::Filter(_) => { unimplemented!() }
            ContextEvent::Inner(i_event) => {
                i_event.handle_event(&self);
            }
        }
    }
}