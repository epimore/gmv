use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::FmtMuxer;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::muxer::MuxerContext;
use crate::media::context::utils::codecpar::repair_basic_stream_info;
use crate::media::rtp::RtpPacketBuffer;
use crate::state::layer::muxer_layer::MuxerLayer;
use crate::state::msg::StreamConfig;
use base::bus::mpsc::TypedReceiver;
use base::exception::typed::common::MessageBusError;
use base::exception::{GlobalError, GlobalResult};
use log::error;
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::AVPacket;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO, av_rescale_q,
};
use shared::info::media_info_ext::MediaExt;
use std::collections::VecDeque;

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
        };
        Ok((context, converter.muxer))
    }
    unsafe fn fix_basic_stream_info(&mut self) -> GlobalResult<VecDeque<AVPacket>> {
        let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
        let ext = &self.media_ext;
        let params = &mut self.demuxer_context.params;
        let mut pkts = VecDeque::with_capacity(128);
        let mut counter = 128;

        while counter > 0 {
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
            if ret < 0 {
                return Ok(pkts);
            }
            let mut all_ready = true;
            for (i, param) in params.iter_mut().enumerate().map(|(i, param)| (i, param)) {
                let st = *(*fmt_ctx).streams.offset(i as isize);
                let codecpar = (*st).codecpar;
                if matches!(
                    (*codecpar).codec_type,
                    AVMediaType_AVMEDIA_TYPE_VIDEO | AVMediaType_AVMEDIA_TYPE_AUDIO
                ) {
                    param.ready = repair_basic_stream_info(st, &pkt, ext, param);
                } else {
                    param.ready = true;
                }
                all_ready = all_ready && param.ready;
            }
            pkts.push_back(pkt);
            if all_ready {
                break;
            }
            counter -= 1;
        }
        Ok(pkts)
    }

    pub fn invoke(&mut self, muxer_layer: MuxerLayer) -> GlobalResult<()> {
        unsafe {
            //write init
            let mut cache_pkts = self.fix_basic_stream_info()?;
            //流结束
            if cache_pkts.is_empty() {
                return Ok(());
            }
            self.muxer_context = MuxerContext::init(&self.demuxer_context, &muxer_layer);
            let mut first_pts = 0;
            let mut first_dts = 0;
            if let Some(mut first_pkt) = cache_pkts.pop_front() {
                first_pts = first_pkt.pts;
                first_dts = first_pkt.dts;
                self.process(first_dts, first_pts, &mut first_pkt)?;
                rsmpeg::ffi::av_packet_unref(&mut first_pkt);
            }
            while let Some(mut pkt) = cache_pkts.pop_front() {
                self.process(first_dts, first_pts, &mut pkt)?;
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }

            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            //write body
            let mut pkt = std::mem::zeroed::<AVPacket>();
            loop {
                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if ret < 0 {
                    break;
                }
                self.process(first_dts, first_pts, &mut pkt)?;
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
            //write end
            Self::handle_pkt_muxer_end(&mut self.muxer_context);
        }

        fn rpt_diff_u32(a: u32, b: u32) -> u32 {
            if a >= b { a - b } else { b.wrapping_sub(a) }
        }
        Ok(())
    }
    unsafe fn process(
        &mut self,
        first_pts: i64,
        first_dts: i64,
        pkt: &mut AVPacket,
    ) -> GlobalResult<()> {
        let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
        match self.context_event_rx.try_recv() {
            Ok(event) => self.handle_event(event),
            Err(MessageBusError::ChannelClosed) => {
                return Err(GlobalError::new_sys_error(
                    "数据已释放，通道关闭",
                    |msg| error!("{msg}"),
                ));
            }
            Err(_) => {}
        }
        let rtp_state = &mut *self.rtp_state;
        let (cur_unwrapped, duration_ticks) =
            rtp_state.update(rtp_state.timestamp, self.media_ext.clock_rate as u32);
        // //fill_stream_from_media_ext(st, media_ext);
        let tb = (*(*fmt_ctx).streams.offset(pkt.stream_index as isize).read()).time_base;
        // // 更新 RTP 状态并获取展开 timestamp 和帧间差值

        let rtp_tb = AVRational {
            num: 1,
            den: self.media_ext.clock_rate,
        };
        let pts_rescaled = av_rescale_q(cur_unwrapped, rtp_tb, tb);
        // let duration_rescaled = if duration_ticks > 0 {
        //     av_rescale_q(duration_ticks, rtp_tb, tb)
        // } else {
        //     av_rescale_q((self.media_ext.clock_rate / 25) as i64, rtp_tb, tb)
        // };
        // pkt.duration = duration_rescaled;

        // 通过 pts 计算累计真实时长（秒）
        let real_ts = pts_rescaled as f64 * tb.num as f64 / tb.den as f64;
        pkt.pts = pkt.pts - first_pts;
        pkt.dts = pkt.dts - first_dts;
        println!(
            "Packet : stream={}, dts={}, pts={}, duration={}, size={}, key={},timestamp={}",
            pkt.stream_index,
            pkt.dts,
            pkt.pts,
            pkt.duration,
            pkt.size,
            (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0,
            real_ts
        );
        // 暂不实现处理codec
        // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
        // 暂不实现处理filter
        // Self::handle_filter(&mut self.filter_context);

        // 调用 muxer
        Self::handle_pkt_muxer(&mut self.muxer_context, &pkt, real_ts as u64);
        Ok(())
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
