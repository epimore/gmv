use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::FmtMuxer;
use crate::media::context::format::dashmp4::DashCmafMp4Context;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::fmp4::CmafFmp4Context;
use crate::media::context::format::hlsfmp4::HlsFmp4Context;
use crate::media::context::format::muxer::MuxerContext;
use crate::media::context::utils::codecpar::repair_basic_stream_info;
use crate::media::context::utils::extradata::dump_stream_info;
use crate::media::context::utils::time_scale::{ProcessResult, TimelineNormalizer};
use crate::media::rtp::RtpPacketBuffer;
use crate::state::layer::muxer_layer::MuxerLayer;
use crate::state::msg::StreamConfig;
use base::bus::mpsc::TypedReceiver;
use base::chrono::Local;
use base::exception::typed::common::MessageBusError;
use base::exception::{BizError, GlobalError, GlobalResult};
use log::{error, info, warn};
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO, av_rescale_q,
};
use rsmpeg::ffi::{AVMediaType, AVPacket};
use shared::info::media_info_ext::MediaExt;
use std::collections::VecDeque;
use std::ffi::c_int;
use std::time::Instant;

mod codec;
pub mod event;
mod filter;
pub mod format;
pub mod utils;

/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步

pub struct RtpState {
    pub first_unwrapped: i64,
    pub timestamp: u32, // 读取rtp包的timestamp
    pub marker: bool,   // 读取rtp包的mark

    pub last_32: u32,        // 上一次 RTP timestamp（32-bit）
    pub last_unwrapped: i64, // 上一次展开 timestamp，用于累积 diff
}
impl RtpState {
    pub fn new() -> Self {
        Self {
            first_unwrapped: 0,
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
            let max_diff = clock_rate as i64 * 3; // 3 秒最大 diff
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
//idr帧及以后开始缓存
struct InitCacheInfo {
    //(rtp_ts累计,duration_ticks,pkt)
    pkts: VecDeque<(i64, i64, AVPacket)>,
    base_dts_pts: Vec<(i64, i64)>,
}
impl MediaContext {
    /// 判断是否有视频流
    fn has_video_stream(&self) -> (bool, usize) {
        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            if fmt_ctx.is_null() {
                return (false, 0);
            }

            let nb_streams = (*fmt_ctx).nb_streams as usize;
            for i in 0..nb_streams {
                let stream = *(*fmt_ctx).streams.add(i);
                let codecpar = (*stream).codecpar;

                if !codecpar.is_null() && (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
                    return (true, i);
                }
            }
        }
        (false, 0)
    }

    /// 判断是否有音频流
    fn has_audio_stream(&self) -> (bool, usize) {
        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            if fmt_ctx.is_null() {
                return (false, 0);
            }

            let nb_streams = (*fmt_ctx).nb_streams as usize;
            for i in 0..nb_streams {
                let stream = *(*fmt_ctx).streams.add(i);
                let codecpar = (*stream).codecpar;

                if !codecpar.is_null() && (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
                    return (true, i);
                }
            }
        }
        (false, 0)
    }

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
    //读取数据帧补充修复流信息
    unsafe fn fix_basic_stream_info(&mut self) -> GlobalResult<InitCacheInfo> {
        // dump_stream_info(&self.demuxer_context);
        let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
        // 判断是否有视频流
        let (has_video, v_idx) = self.has_video_stream();
        let (has_audio, a_idx) = self.has_audio_stream();
        let ext = &self.media_ext;
        let params = &mut self.demuxer_context.params;
        let mut cache_info = InitCacheInfo {
            pkts: Default::default(),
            base_dts_pts: Default::default(),
        };
        let mut audio_ready = false;
        let mut counter = 128;
        let mut init_first_rtp_time = false;
        let mut video_keyframe_found = false;
        while counter > 0 {
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
            if ret < 0 {
                Err(GlobalError::new_sys_error("pkt end or err", |msg| {
                    warn!("{msg}")
                }))?;
            }
            let idx = pkt.stream_index as usize;
            if idx >= params.len() {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            let rtp_state = &mut *self.rtp_state;
            let (cur_unwrapped, duration_ticks) =
                rtp_state.update(rtp_state.timestamp, self.media_ext.clock_rate as u32);
            if !init_first_rtp_time {
                rtp_state.first_unwrapped = cur_unwrapped;
                init_first_rtp_time = true;
            }
            let st = *(*fmt_ctx).streams.offset(idx as isize);
            let codecpar = (*st).codecpar;
            // 修复流信息
            match (*codecpar).codec_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    if !params[idx].ready {
                        let ready = repair_basic_stream_info(st, &pkt, ext, &mut params[idx]);
                        params[idx].ready = ready;
                    }
                    if !video_keyframe_found && pkt.flags & AV_PKT_FLAG_KEY as i32 != 0 {
                        video_keyframe_found = true;
                        let dts = pkt.dts;
                        let pts = pkt.pts;
                        cache_info.base_dts_pts[idx] = (dts, pts);
                    }
                }
                AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    if !params[idx].ready {
                        let ready = repair_basic_stream_info(st, &pkt, ext, &mut params[idx]);
                        params[idx].ready = ready;
                        if has_video
                            && video_keyframe_found
                            && cache_info.base_dts_pts.get(idx).is_none()
                        {
                            let dts = pkt.dts;
                            let pts = pkt.pts;
                            cache_info.base_dts_pts[idx] = (dts, pts);
                        } else if !has_video && cache_info.base_dts_pts.get(idx).is_none() {
                            let dts = pkt.dts;
                            let pts = pkt.pts;
                            cache_info.base_dts_pts[idx] = (dts, pts);
                        }
                    }
                    audio_ready = true;
                }
                _ => {
                    if has_video
                        && video_keyframe_found
                        && cache_info.base_dts_pts.get(idx).is_none()
                    {
                        let dts = pkt.dts;
                        let pts = pkt.pts;
                        cache_info.base_dts_pts[idx] = (dts, pts);
                    } else if !has_video && cache_info.base_dts_pts.get(idx).is_none() {
                        let dts = pkt.dts;
                        let pts = pkt.pts;
                        cache_info.base_dts_pts[idx] = (dts, pts);
                    }
                    params[idx].ready = true;
                }
            }
            // 决定是否开始缓存
            let should_cache = if has_video {
                video_keyframe_found // 有视频流时，等待关键帧
            } else {
                audio_ready // 只有音频流时，有音频数据就开始
            };
            if should_cache {
                cache_info
                    .pkts
                    .push_back((cur_unwrapped, duration_ticks, pkt));
            } else {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
            let all_ready = params.iter().all(|ready| ready.ready);
            if all_ready && should_cache {
                break;
            }
            counter -= 1;
        }
        dump_stream_info(&self.demuxer_context);
        if has_video && cache_info.base_dts_pts.get(v_idx).is_none() {
            Err(GlobalError::new_sys_error(
                "pkt has video,but stream miss",
                |msg| warn!("{msg}"),
            ))?;
        }
        if has_audio && cache_info.base_dts_pts.get(a_idx).is_none() {
            Err(GlobalError::new_sys_error(
                "pkt has audio,but stream miss",
                |msg| warn!("{msg}"),
            ))?;
        }
        for i in 0..cache_info.base_dts_pts.len() {
            if cache_info.base_dts_pts.get(i).is_none() {
                if has_video {
                    cache_info.base_dts_pts[i] = cache_info.base_dts_pts[v_idx];
                } else {
                    cache_info.base_dts_pts[i] = cache_info.base_dts_pts[a_idx];
                }
            }
        }

        Ok(cache_info)
    }

    pub fn invoke(&mut self, muxer_layer: MuxerLayer) -> GlobalResult<()> {
        unsafe {
            //修复流信息
            let mut cache_info = self.fix_basic_stream_info()?;
            //流结束
            if cache_info.pkts.is_empty() {
                return Ok(());
            }
            let nb_streams = self.demuxer_context.params.len();
            let mut normalizer = TimelineNormalizer::new(nb_streams);
            for i in 0..nb_streams {
                let stream = *(*self.demuxer_context.avio.fmt_ctx).streams.add(i);
                let media_type = (*stream).codecpar.as_ref().unwrap().codec_type;
                normalizer.init_stream(i, media_type);
            }
            //初始化muxer
            self.muxer_context = MuxerContext::init(&self.demuxer_context, muxer_layer);
            // 记录第一个包的 RTP 时间
            let first_rtp_time = cache_info.pkts.front().map(|(ts, _, _)| *ts).unwrap_or(0);
            //消费缓存数据，以关键帧开始
            while let Some((cur_unwrapped, duration_ticks, mut pkt)) = cache_info.pkts.pop_front() {
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
                self.process(
                    cur_unwrapped,
                    duration_ticks,
                    first_rtp_time,
                    &mut normalizer,
                    &mut pkt,
                )?;
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;

            //write body
            loop {
                match self.context_event_rx.try_recv() {
                    Ok(event) => self.handle_event(event),
                    Err(MessageBusError::ChannelClosed) => {
                        info!("ssrc = {} ;释放资源", self.ssrc);
                        return Ok(());
                    }
                    Err(_) => {}
                }
                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if ret < 0 {
                    break;
                }

                let rtp_state = &mut *self.rtp_state;
                let first_unwrapped = rtp_state.first_unwrapped;
                let (cur_unwrapped, duration_ticks) =
                    rtp_state.update(rtp_state.timestamp, self.media_ext.clock_rate as u32);
                self.process(
                    cur_unwrapped,
                    duration_ticks,
                    first_unwrapped,
                    &mut normalizer,
                    &mut pkt,
                )?;
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
        _cur_unwrapped: i64,
        _duration_ticks: i64,
        _first_unwrapped: i64,
        normalizer: &mut TimelineNormalizer,
        pkt: &mut AVPacket,
    ) -> GlobalResult<()> {
        if let (Some(master_clock_us), res) = normalizer.process(pkt) {
            /*

            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;

            let stream_tb =
                (*(*fmt_ctx).streams.offset(pkt.stream_index as isize).read()).time_base;
            // // 更新 RTP 状态并获取展开 timestamp 和帧间差值
            let relative_time = cur_unwrapped - first_unwrapped;
            let rtp_tb = AVRational {
                num: 1,
                den: self.media_ext.clock_rate,
            };

            let pts_rescaled = av_rescale_q(relative_time, rtp_tb, stream_tb);
            // 通过 pts 计算累计真实时长（秒）
            let real_ts = pts_rescaled as f64 * stream_tb.num as f64 / stream_tb.den as f64;*/

            // 暂不实现处理codec
            // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
            // 暂不实现处理filter
            // Self::handle_filter(&mut self.filter_context);

            // 调用 muxer 其中master_clock_us需要转换为秒，供录制进度信息
            Self::handle_pkt_muxer(self, res, &pkt, (master_clock_us / 1000_000) as u64);
        }
        Ok(())
    }
    fn handle_codec(codec: &mut CodecContext) {}
    fn handle_filter(filter: &mut FilterContext) {}

    // 1.写入头信息
    // 2.循环写入body
    // 3.写入结束信息
    // 问题如何传递信息【该使用写入结束信息】
    // 回调
    fn handle_pkt_muxer(&mut self, epoch: ProcessResult, pkt: &AVPacket, ts: u64) {
        let muxer = &mut self.muxer_context;
        if let Some(context) = &mut muxer.flv {
            let _ = context.write_packet(pkt, ts);
        }
        if let Some(context) = &mut muxer.mp4 {
            let _ = context.write_packet(pkt, ts);
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
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            let _ = context.write_packet(pkt, ts);
        }
        if let Some(context) = &mut muxer.dash_mp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            let _ = context.write_packet(pkt, ts);
        }
        if let Some(context) = &mut muxer.hls_mp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            let _ = context.write_packet(pkt, ts);
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
        if let Some(context) = &mut muxer.dash_mp4 {
            context.flush();
        }
        if let Some(context) = &mut muxer.hls_mp4 {
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
