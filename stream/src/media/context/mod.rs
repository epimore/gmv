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
use base::log::warn;
use shared::info::media_info_ext::MediaExt;
use std::time::Instant;

pub mod event;
pub mod format;
mod codec;
mod filter;
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
        use std::time::Instant;

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

                // --- RTP timestamp 解包与平滑映射 ---
                // rtp_state 是裸指针，先转成可变引用
                let rtp_state = &mut *self.rtp_state;

                // 当前 RTP timestamp（来自 rtp layer，单位：90kHz ticks）
                let cur_ts_32 = rtp_state.timestamp;
                // 处理 wrap：如果当前低 32 位小于 last_32 且差距很大 => 增加 wraps
                if rtp_state.initialized {
                    // detect wrap-around: if cur < last_32 and difference > 2^31 roughly
                    if cur_ts_32 < rtp_state.last_32 && (rpt_diff_u32(rtp_state.last_32, cur_ts_32) > 0x8000_0000u32) {
                        rtp_state.wraps = rtp_state.wraps.wrapping_add(1);
                    }
                } else {
                    // 首包初始化 last_32
                    rtp_state.last_32 = cur_ts_32;
                    rtp_state.wraps = 0;
                }

                // 完整展开成 64-bit
                let mut cur_unwrapped = (rtp_state.wraps as u128 * 0x1_0000_0000u128) + (cur_ts_32 as u128);
                let mut cur_unwrapped_i64 = cur_unwrapped as i64;

                // 平滑：基于 wall-clock 估算 expected increment（90 ticks/ms）
                let now = Instant::now();
                let delta_wall_ms = now.duration_since(rtp_state.last_arrival).as_millis() as i64;
                // expected increment in RTP ticks (90 ticks per ms)
                let expected_inc = delta_wall_ms.saturating_mul(90);

                if rtp_state.initialized {
                    let last = rtp_state.last_unwrapped;
                    let actual_inc = cur_unwrapped_i64.saturating_sub(last);

                    // 允许一定倍数（例如 5x）或最小阈值
                    let allowed = std::cmp::max(expected_inc.saturating_mul(5), 9000); // 最少 9000 ticks ~100ms 容差
                    if expected_inc > 0 && actual_inc > allowed {
                        // 前向突发跳变，限幅为 expected_inc（避免 pts 突增）
                        cur_unwrapped_i64 = last.saturating_add(expected_inc);
                    } else if expected_inc > 0 && actual_inc < -allowed {
                        // 非法向后跳（极端），限制回退
                        cur_unwrapped_i64 = last.saturating_sub(expected_inc);
                    }
                } else {
                    // 首次到来：标记为已初始化
                    rtp_state.initialized = true;
                }

                // 把展开值存回 state 基础字段
                rtp_state.last_unwrapped = cur_unwrapped_i64;
                rtp_state.last_32 = cur_ts_32;
                rtp_state.last_arrival = now;

                // --- 把 RTP(90k) 映射到流的 time_base ---
                // 使用 av_rescale_q: from (1/90000) -> tb
                let rtp_tb = AVRational { num: 1, den: self.media_ext.clock_rate };
                let pts_rescaled = av_rescale_q(cur_unwrapped_i64 as i64, rtp_tb, tb);

                // 计算 duration：若有上帧则用差值，否则用 expected_inc 估计
                let mut duration = 0i64;
                if rtp_state.last_pts != 0 {
                    duration = pts_rescaled.saturating_sub(rtp_state.last_pts);
                    if duration <= 0 {
                        // 如果计算出非正值（异常），用期望增量估计
                        let est = av_rescale_q(expected_inc as i64, rtp_tb, tb);
                        duration = std::cmp::max(1, est);
                    }
                } else {
                    // 首帧：用 estimated frame duration（比如按到达时间估算）
                    let est = av_rescale_q(expected_inc as i64, rtp_tb, tb);
                    duration = std::cmp::max(1, est);
                }

                // 更新 last_pts（用于下一帧 duration）
                rtp_state.last_pts = pts_rescaled;

                // 将 pts/dts/duration 写回 pkt（注意类型：AVPacket 的字段是 i64）
                pkt.dts = pts_rescaled;
                pkt.pts = pts_rescaled;
                pkt.duration = duration as i64;

                // 处理muxer（内部会 clone pkt）
                Self::handle_muxer(&mut self.muxer_context, &pkt);

                // 清理
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
        }

        // helper: difference of two u32 in unsigned sense (a - b)
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
        if let Some(mpr_context) = &muxer.mp4 { unimplemented!() }
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