use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::FmtMuxer;
use crate::media::context::format::dashmp4::DashCmafMp4Context;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::flv::FlvSupperCtx;
use crate::media::context::format::fmp4::CmafFmp4Context;
use crate::media::context::format::hlsfmp4::HlsFmp4Context;
use crate::media::context::format::muxer::MuxerContext;
use crate::media::context::utils::codecpar::repair_basic_stream_info;
use crate::media::context::utils::extradata::{self, dump_stream_info};
use crate::media::context::utils::time_scale::{
    ProcessResult, TimelineNormalizer, repair_missing_timestamps,
};
use crate::media::rtp::RtpPacketBuffer;
use crate::media::show_ffmpeg_error_msg;
use crate::state::layer::muxer_layer::MuxerLayer;
use crate::state::msg::StreamConfig;
use crate::state::register::{
    ActualMediaProfile, OutputMediaMetadata, OutputRuntimeState, Register,
};
use base::bus::mpsc::TypedReceiver;
use base::bytes::BytesMut;
use base::chrono::Local;
use base::err::BaseErrorCode;
use base::exception::typed::common::MessageBusError;
use base::exception::{GlobalError, GlobalResult};
use gmv_domain::info::media_info_ext::MediaExt;
use log::{error, warn};
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVERROR_EOF, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO,
    av_rescale_q,
};
use rsmpeg::ffi::{AVMediaType, AVPacket};
use std::collections::VecDeque;
use std::ffi::c_int;
use std::sync::Arc;
use std::time::Instant;

mod codec;
pub mod event;
mod filter;
pub mod format;
pub mod utils;

/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步
const FIX_MAX_READ_FRAME: usize = 128;
const TOPOLOGY_SETTLE_PACKETS: usize = 64;
const TOPOLOGY_SETTLE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCompletion {
    Eof,
    InputClosed,
}

#[derive(Debug)]
pub enum MediaRunError {
    Ffmpeg {
        stage: &'static str,
        code: c_int,
        message: String,
    },
    Pipeline(GlobalError),
}

impl From<GlobalError> for MediaRunError {
    fn from(error: GlobalError) -> Self {
        Self::Pipeline(error)
    }
}

fn classify_read_frame(ret: c_int, stage: &'static str) -> Result<bool, MediaRunError> {
    if ret >= 0 {
        Ok(true)
    } else if ret == AVERROR_EOF {
        Ok(false)
    } else {
        Err(MediaRunError::Ffmpeg {
            stage,
            code: ret,
            message: show_ffmpeg_error_msg(ret),
        })
    }
}

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
    pub stream_id: Option<Arc<str>>,
    pub media_ext: MediaExt,
    pub codec_context: Option<CodecContext>,
    pub filter_context: FilterContext,
    pub muxer_context: MuxerContext,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub demuxer_context: DemuxerContext,
    pub rtp_state: *mut RtpState,
    actual_media_profile: Option<ActualMediaProfile>,
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
    pkts: VecDeque<AVPacket>,
    timeline_normalizer: TimelineNormalizer,
}

#[derive(Default)]
struct TopologySettleState {
    packets_since_change: usize,
    bytes_since_change: usize,
}

impl TopologySettleState {
    fn observe_packet(&mut self, supported_stream_discovered: bool, packet_size: usize) {
        if supported_stream_discovered {
            self.packets_since_change = 0;
            self.bytes_since_change = 0;
        } else {
            self.packets_since_change = self.packets_since_change.saturating_add(1);
            self.bytes_since_change = self.bytes_since_change.saturating_add(packet_size);
        }
    }

    fn is_stable(&self) -> bool {
        self.packets_since_change > TOPOLOGY_SETTLE_PACKETS
            && self.bytes_since_change > TOPOLOGY_SETTLE_BYTES
    }
}

fn is_supported_av(media_type: AVMediaType) -> bool {
    matches!(
        media_type,
        AVMediaType_AVMEDIA_TYPE_VIDEO | AVMediaType_AVMEDIA_TYPE_AUDIO
    )
}

fn supported_stream_params_ready(streams: impl Iterator<Item = (AVMediaType, bool)>) -> bool {
    streams
        .filter(|(media_type, _)| is_supported_av(*media_type))
        .all(|(_, ready)| ready)
}

unsafe fn init_stream_timeline(
    fmt_ctx: *mut rsmpeg::ffi::AVFormatContext,
    timeline_normalizer: &mut TimelineNormalizer,
    idx: usize,
) -> Option<AVMediaType> {
    let stream = unsafe { *(*fmt_ctx).streams.add(idx) };
    if stream.is_null() {
        return None;
    }
    let codecpar = unsafe { (*stream).codecpar };
    if codecpar.is_null() {
        return None;
    }
    let media_type = unsafe { (*codecpar).codec_type };
    timeline_normalizer.init_stream(
        idx,
        media_type,
        unsafe { (*stream).time_base },
        unsafe { (*codecpar).codec_id },
        unsafe { (*codecpar).video_delay },
    );
    Some(media_type)
}

unsafe fn sync_discovered_streams(
    demuxer_context: &mut DemuxerContext,
    timeline_normalizer: &mut TimelineNormalizer,
) -> usize {
    let fmt_ctx = demuxer_context.avio.fmt_ctx;
    demuxer_context.sync_params();
    let mut supported_streams_discovered = 0;
    for idx in 0..demuxer_context.params.len() {
        if timeline_normalizer.is_stream_initialized(idx) {
            continue;
        }
        if unsafe { init_stream_timeline(fmt_ctx, timeline_normalizer, idx) }
            .is_some_and(is_supported_av)
        {
            supported_streams_discovered += 1;
        }
    }
    supported_streams_discovered
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
        let rtp_buffer =
            RtpPacketBuffer::init(ssrc, stream_config.rtp_rx, &stream_config.media_ext);
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
            codec_context: CodecContext::init(converter.codec, converter.transcode),
            filter_context: FilterContext::init(converter.filter),
            ssrc,
            stream_id: Register::stream_id_by_ssrc(ssrc),
            media_ext: stream_config.media_ext,
            context_event_rx: stream_config.context_event_rx,
            muxer_context: Default::default(),
            demuxer_context,
            rtp_state: rtp_state_ptr,
            actual_media_profile: None,
        };
        Ok((context, converter.muxer))
    }
    //读取数据帧补充修复流信息
    unsafe fn fix_basic_stream_info(&mut self) -> Result<InitCacheInfo, MediaRunError> {
        let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
        let ext = &self.media_ext;
        let mut cache_info = InitCacheInfo {
            pkts: VecDeque::new(),
            timeline_normalizer: TimelineNormalizer::new(0),
        };
        for i in 0..self.demuxer_context.params.len() {
            unsafe {
                init_stream_timeline(fmt_ctx, &mut cache_info.timeline_normalizer, i);
            }
        }
        let mut video_keyframe_found = false;
        let mut audio_ready = false;
        let mut topology_settle = TopologySettleState::default();

        let mut counter = 0;
        while counter < FIX_MAX_READ_FRAME {
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
            if !classify_read_frame(ret, "fix_basic_stream_info")? {
                break;
            }
            counter += 1;

            let supported_streams_discovered = unsafe {
                sync_discovered_streams(
                    &mut self.demuxer_context,
                    &mut cache_info.timeline_normalizer,
                )
            };
            topology_settle
                .observe_packet(supported_streams_discovered > 0, pkt.size.max(0) as usize);

            if pkt.stream_index < 0 {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            let idx = pkt.stream_index as usize;
            if idx >= self.demuxer_context.params.len() {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            let st = *(*fmt_ctx).streams.add(idx);
            let codecpar = (*st).codecpar;
            if codecpar.is_null() {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            if pkt.data.is_null() || pkt.size <= 0 {
                warn!(
                    "Discard empty packet; ssrc: {}, pts: {}, dts: {} key frame: {}",
                    self.ssrc,
                    pkt.pts,
                    pkt.dts,
                    (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO
                        && pkt.flags & AV_PKT_FLAG_KEY as i32 != 0
                );
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            // 统一修复流信息
            if !repair_missing_timestamps(&mut pkt, (*codecpar).video_delay) {
                warn!("Discard packet without pts/dts; ssrc: {}", self.ssrc);
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            let param = &mut self.demuxer_context.params[idx];
            if !param.ready {
                param.ready = repair_basic_stream_info(st, &pkt, ext, param);
            }
            // 标记状态
            match (*codecpar).codec_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    if pkt.flags & AV_PKT_FLAG_KEY as i32 != 0 {
                        video_keyframe_found = true;
                    }
                }
                AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    audio_ready = true;
                }
                _ => {}
            }

            // 起播条件
            let should_cache = if self.has_video_stream().0 {
                video_keyframe_found
            } else {
                audio_ready
            };

            if should_cache {
                cache_info
                    .timeline_normalizer
                    .rescale_global_base_us(idx, pkt.pts.min(pkt.dts));
                cache_info.pkts.push_back(pkt);
            } else {
                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }

            let params = &self.demuxer_context.params;
            let supported_params_ready =
                supported_stream_params_ready((0..params.len()).filter_map(|idx| {
                    let stream = *(*fmt_ctx).streams.add(idx);
                    let codecpar = stream.as_ref()?.codecpar.as_ref()?;
                    Some((codecpar.codec_type, params[idx].ready))
                }));
            if should_cache && supported_params_ready && topology_settle.is_stable() {
                break;
            }
        }

        if cache_info.timeline_normalizer.global_base_us == i64::MAX {
            cache_info.timeline_normalizer.global_base_us = 0
        };
        dump_stream_info(&self.demuxer_context);

        Ok(cache_info)
    }

    pub fn invoke(&mut self, muxer_layer: MuxerLayer) -> Result<MediaCompletion, MediaRunError> {
        unsafe {
            //修复流信息
            let mut cache_info = self.fix_basic_stream_info()?;
            //流结束
            if cache_info.pkts.is_empty() {
                return Ok(MediaCompletion::Eof);
            }
            let mut normalizer = &mut cache_info.timeline_normalizer;
            if let Some(codec) = &mut self.codec_context {
                codec.prepare(&mut self.demuxer_context)?;
            }
            let media_param = extradata::parse_media_param(&self.demuxer_context);
            let actual_media_profile = ActualMediaProfile {
                video_codec: media_param
                    .video
                    .as_ref()
                    .map(|video| video.codec.clone())
                    .unwrap_or_default(),
                audio_codec: media_param
                    .audio
                    .as_ref()
                    .map(|audio| audio.codec.clone())
                    .unwrap_or_default(),
            };
            if let Some(stream_id) = self.stream_id.as_deref() {
                Register::set_actual_media_profile(stream_id, actual_media_profile.clone())?;
            }
            self.actual_media_profile = Some(actual_media_profile);
            //初始化muxer
            self.muxer_context = MuxerContext::init(&self.demuxer_context, muxer_layer)?;
            //消费缓存数据，以关键帧开始
            while let Some(mut pkt) = cache_info.pkts.pop_front() {
                match self.context_event_rx.try_recv() {
                    Ok(event) => self.handle_event(event)?,
                    Err(MessageBusError::ChannelClosed) => {
                        rsmpeg::ffi::av_packet_unref(&mut pkt);
                        self.finish_pipeline()?;
                        return Ok(MediaCompletion::InputClosed);
                    }
                    Err(_) => {}
                }
                let process_result = self.process(&mut normalizer, &mut pkt);
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                process_result?;
            }
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;

            //write body
            loop {
                match self.context_event_rx.try_recv() {
                    Ok(event) => self.handle_event(event)?,
                    Err(MessageBusError::ChannelClosed) => {
                        self.finish_pipeline()?;
                        return Ok(MediaCompletion::InputClosed);
                    }
                    Err(_) => {}
                }
                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if !classify_read_frame(ret, "read_frame")? {
                    break;
                }
                if let Err(error) = self.ensure_output_topology_unchanged() {
                    rsmpeg::ffi::av_packet_unref(&mut pkt);
                    return Err(error.into());
                }
                if pkt.stream_index < 0
                    || pkt.stream_index as usize >= self.demuxer_context.params.len()
                {
                    rsmpeg::ffi::av_packet_unref(&mut pkt);
                    continue;
                }

                // let rtp_state = &mut *self.rtp_state;
                // let first_unwrapped = rtp_state.first_unwrapped;
                // let (cur_unwrapped, duration_ticks) =
                //     rtp_state.update(rtp_state.timestamp, self.media_ext.clock_rate as u32);
                let process_result = self.process(&mut normalizer, &mut pkt);
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                process_result?;
            }
            //write end
            self.finish_pipeline()?;
        }

        fn rpt_diff_u32(a: u32, b: u32) -> u32 {
            if a >= b { a - b } else { b.wrapping_sub(a) }
        }
        Ok(MediaCompletion::Eof)
    }

    fn finish_pipeline(&mut self) -> GlobalResult<()> {
        let packets = match &mut self.codec_context {
            Some(codec) => codec.flush()?,
            None => Vec::new(),
        };
        for packet in packets {
            Self::handle_pkt_muxer(
                self,
                ProcessResult::Ok,
                &packet,
                (packet.pts.max(0) as u64) / 48_000,
            )?;
        }
        Self::handle_pkt_muxer_end(&mut self.muxer_context);
        Ok(())
    }
    unsafe fn process(
        &mut self,
        normalizer: &mut TimelineNormalizer,
        pkt: &mut AVPacket,
    ) -> GlobalResult<()> {
        if let (Some(master_clock_us), res) = normalizer.process(pkt, self.ssrc) {
            // 暂不实现处理codec
            // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
            // 暂不实现处理filter
            // Self::handle_filter(&mut self.filter_context);
            // 调用 muxer 其中master_clock_us需要转换为秒，供录制进度信息
            if self
                .codec_context
                .as_ref()
                .is_some_and(|codec| codec.handles(pkt))
            {
                let packets = self
                    .codec_context
                    .as_mut()
                    .expect("codec context checked")
                    .process(pkt)?;
                for packet in packets {
                    Self::handle_pkt_muxer(
                        self,
                        res,
                        &packet,
                        (packet.pts.max(0) as u64) / 48_000,
                    )?;
                }
            } else {
                Self::handle_pkt_muxer(self, res, &pkt, (master_clock_us / 1000_000) as u64)?;
            }
        }
        Ok(())
    }

    unsafe fn ensure_output_topology_unchanged(&self) -> GlobalResult<()> {
        let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
        let initialized_stream_count = self.demuxer_context.params.len();
        let current_stream_count = (*fmt_ctx).nb_streams as usize;
        for idx in initialized_stream_count..current_stream_count {
            let stream = *(*fmt_ctx).streams.add(idx);
            let Some(stream) = stream.as_ref() else {
                continue;
            };
            let Some(codecpar) = stream.codecpar.as_ref() else {
                continue;
            };
            if is_supported_av(codecpar.codec_type) {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    &format!(
                        "stream topology changed after muxer initialization: previous_stream_count={}, current_stream_count={}",
                        initialized_stream_count, current_stream_count
                    ),
                    |msg| error!("{msg}"),
                ));
            }
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
    fn handle_pkt_muxer(
        &mut self,
        epoch: ProcessResult,
        pkt: &AVPacket,
        ts: u64,
    ) -> GlobalResult<()> {
        let muxer = &mut self.muxer_context;
        if let Some(context) = &mut muxer.flv {
            match context {
                FlvSupperCtx::FlvCtx(context) => {
                    context.write_packet(pkt, ts)?;
                }
                FlvSupperCtx::H265FlvCtx(context) => {
                    context.write_packet(pkt, ts)?;
                }
            }
        }
        if let Some(context) = &mut muxer.mp4 {
            context.write_packet(pkt, ts)?;
        }
        if muxer.ts.is_some() {
            warn!("stream packet mux ignored unsupported ts output");
        }
        if muxer.rtp_frame.is_some() {
            warn!("stream packet mux ignored unsupported rtp-frame output");
        }
        if muxer.rtp_ps.is_some() {
            warn!("stream packet mux ignored unsupported rtp-ps output");
        }
        if muxer.rtp_enc.is_some() {
            warn!("stream packet mux ignored unsupported rtp-enc output");
        }
        if muxer.hls_ts.is_some() {
            warn!("stream packet mux ignored unsupported hls-ts output");
        }
        if let Some(context) = &mut muxer.fmp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            context.write_packet(pkt, ts)?;
        }
        if let Some(context) = &mut muxer.dash_mp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            context.write_packet(pkt, ts)?;
        }
        if epoch == ProcessResult::Discontinuity {
            if let Some(mut context) = muxer.hls_mp4.take() {
                let pkt_tx = context.pkt_tx.clone();
                context.flush();
                let next_segment_seq = context.next_segment_seq();
                let mut context = HlsFmp4Context::init_context(&self.demuxer_context, pkt_tx)?;
                context.set_segment_seq(next_segment_seq);
                muxer.hls_mp4 = Some(context);
            }
        }
        if let Some(context) = &mut muxer.hls_mp4 {
            context.write_packet(pkt, ts)?;
        }
        self.sync_output_readiness()?;
        Ok(())
    }

    fn sync_output_readiness(&self) -> GlobalResult<()> {
        let Some(stream_id) = self.stream_id.as_deref() else {
            return Ok(());
        };
        let Some(profile) = self.actual_media_profile.as_ref() else {
            return Ok(());
        };
        let muxer = &self.muxer_context;
        let flv_published = match muxer.flv.as_ref() {
            Some(FlvSupperCtx::FlvCtx(context)) => context.pkt_tx.has_published(),
            Some(FlvSupperCtx::H265FlvCtx(context)) => context.tx.has_published(),
            None => false,
        };
        if flv_published {
            set_output_ready(stream_id, "flv", profile)?;
        }
        if muxer
            .fmp4
            .as_ref()
            .is_some_and(|context| context.pkt_tx.has_published())
        {
            set_output_ready(stream_id, "fmp4", profile)?;
        }
        if muxer
            .hls_mp4
            .as_ref()
            .is_some_and(|context| context.pkt_tx.has_published())
        {
            for output_type in ["hls", "ll_hls"] {
                if Register::output_media_metadata(stream_id, output_type).is_some() {
                    set_output_ready(stream_id, output_type, profile)?;
                }
            }
        }
        Ok(())
    }
    fn handle_pkt_muxer_end(muxer: &mut MuxerContext) {
        if let Some(context) = &mut muxer.flv {
            match context {
                FlvSupperCtx::FlvCtx(context) => {
                    context.flush();
                }
                FlvSupperCtx::H265FlvCtx(context) => {
                    context.flush();
                }
            }
        }
        if let Some(context) = &mut muxer.mp4 {
            context.flush();
        }
        if muxer.ts.is_some() {
            warn!("stream packet mux ignored unsupported ts output");
        }
        if muxer.rtp_frame.is_some() {
            warn!("stream packet mux ignored unsupported rtp-frame output");
        }
        if muxer.rtp_ps.is_some() {
            warn!("stream packet mux ignored unsupported rtp-ps output");
        }
        if muxer.rtp_enc.is_some() {
            warn!("stream packet mux ignored unsupported rtp-enc output");
        }
        if muxer.hls_ts.is_some() {
            warn!("stream packet mux ignored unsupported hls-ts output");
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

    fn handle_event(&mut self, event: ContextEvent) -> GlobalResult<()> {
        match event {
            ContextEvent::Codec(_) => {
                warn!("stream context ignored unsupported codec event");
            }
            ContextEvent::Muxer(m_event) => {
                m_event.handle_event(&mut self.muxer_context, &self.demuxer_context)?;
            }
            ContextEvent::Filter(_) => {
                warn!("stream context ignored unsupported filter event");
            }
            ContextEvent::Inner(i_event) => {
                i_event.handle_event(&self);
            }
        }
        Ok(())
    }
}

fn set_output_ready(
    stream_id: &str,
    output_type: &str,
    profile: &ActualMediaProfile,
) -> GlobalResult<()> {
    if Register::output_media_metadata(stream_id, output_type)
        .is_some_and(|metadata| metadata.state == OutputRuntimeState::Ready)
    {
        return Ok(());
    }
    Register::set_output_media_metadata(
        stream_id,
        output_type,
        OutputMediaMetadata {
            state: OutputRuntimeState::Ready,
            video_codec: profile.video_codec.clone(),
            audio_codec: profile.audio_codec.clone(),
            mime_codec: output_mime_codec(output_type, profile),
        },
    )
}

fn output_mime_codec(output_type: &str, profile: &ActualMediaProfile) -> String {
    if output_type == "flv" {
        return "video/x-flv".to_string();
    }
    let codecs = [profile.video_codec.as_str(), profile.audio_codec.as_str()]
        .into_iter()
        .filter(|codec| !codec.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if codecs.is_empty() {
        String::new()
    } else if profile.video_codec.is_empty() {
        format!("audio/mp4; codecs=\"{codecs}\"")
    } else {
        format!("video/mp4; codecs=\"{codecs}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsmpeg::ffi::AVMediaType_AVMEDIA_TYPE_DATA;

    #[test]
    fn read_frame_classifies_packet_eof_and_failure() {
        assert!(classify_read_frame(0, "test").unwrap());
        assert!(!classify_read_frame(AVERROR_EOF, "test").unwrap());

        match classify_read_frame(-1, "demux") {
            Err(MediaRunError::Ffmpeg { stage, code, .. }) => {
                assert_eq!(stage, "demux");
                assert_eq!(code, -1);
            }
            _ => panic!("non-EOF FFmpeg result must remain a failure"),
        }
    }

    #[test]
    fn topology_settle_window_reads_past_initial_probe_limit() {
        let mut state = TopologySettleState::default();
        for _ in 0..TOPOLOGY_SETTLE_PACKETS {
            state.observe_packet(false, 1024);
        }
        assert!(!state.is_stable());

        state.observe_packet(true, 1024);
        assert!(!state.is_stable());
    }

    #[test]
    fn early_audio_and_video_are_ready_after_settle_window() {
        let streams = [
            (AVMediaType_AVMEDIA_TYPE_VIDEO, true),
            (AVMediaType_AVMEDIA_TYPE_AUDIO, true),
        ];
        let mut state = TopologySettleState::default();
        for _ in 0..=TOPOLOGY_SETTLE_PACKETS {
            state.observe_packet(false, 1024);
        }

        assert!(supported_stream_params_ready(streams.into_iter()));
        assert!(state.is_stable());
    }

    #[test]
    fn late_audio_resets_topology_settle_window() {
        let mut state = TopologySettleState::default();
        for _ in 0..=TOPOLOGY_SETTLE_PACKETS {
            state.observe_packet(false, 1024);
        }
        assert!(state.is_stable());

        state.observe_packet(true, 1024);
        assert!(!state.is_stable());
    }

    #[test]
    fn pure_video_topology_stabilizes_within_read_bound() {
        let mut state = TopologySettleState::default();
        for _ in 0..=TOPOLOGY_SETTLE_PACKETS {
            state.observe_packet(false, 1024);
        }

        assert!(state.is_stable());
        assert!(state.packets_since_change < FIX_MAX_READ_FRAME);
    }

    #[test]
    fn unknown_stream_does_not_block_supported_stream_readiness() {
        let streams = [
            (AVMediaType_AVMEDIA_TYPE_VIDEO, true),
            (AVMediaType_AVMEDIA_TYPE_AUDIO, true),
            (AVMediaType_AVMEDIA_TYPE_DATA, false),
        ];

        assert!(supported_stream_params_ready(streams.into_iter()));
    }

    #[test]
    fn output_mime_uses_actual_audio_and_video_codec_strings() {
        let profile = ActualMediaProfile {
            video_codec: "hev1.1.6.L78".to_string(),
            audio_codec: "mp4a.40.2".to_string(),
        };

        assert_eq!(
            output_mime_codec("fmp4", &profile),
            "video/mp4; codecs=\"hev1.1.6.L78, mp4a.40.2\""
        );
        assert_eq!(output_mime_codec("flv", &profile), "video/x-flv");
    }
}
