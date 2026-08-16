use crate::media::context::codec::{AUDIO_STALL_GRACE_US, CodecContext};
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::FmtMuxer;
use crate::media::context::format::dashmp4::DashCmafMp4Context;
use crate::media::context::format::demuxer::{
    DemuxerContext, OutputTrackSource, ParamRepairState, SYNTHETIC_AUDIO_PACKET_INDEX,
};
use crate::media::context::format::flv::FlvSupperCtx;
use crate::media::context::format::fmp4::CmafFmp4Context;
use crate::media::context::format::hlsfmp4::HlsFmp4Context;
use crate::media::context::format::muxer::{MuxerContext, MuxerEnum};
use crate::media::context::utils::codecpar::{audio_parameters_ready, repair_basic_stream_info};
use crate::media::context::utils::extradata::{self, dump_stream_info};
use crate::media::context::utils::time_scale::{
    ProcessResult, TimelineNormalizer, repair_missing_timestamps,
};
use crate::media::rtp::{RtpInterruptReason, RtpPacketBuffer, RtpReadControl};
use crate::media::show_ffmpeg_error_msg;
use crate::state::layer::muxer_layer::MuxerLayer;
use crate::state::msg::StreamConfig;
use crate::state::register::{
    ActualMediaProfile, AudioRuntimeMetadata, AudioSourceRuntimeState, OutputAudioRuntimeMode,
    OutputMediaMetadata, OutputRuntimeState, Register,
};
use base::bus::mpsc::TypedReceiver;
use base::bytes::BytesMut;
use base::chrono::Local;
use base::exception::typed::common::MessageBusError;
use base::exception::{GlobalError, GlobalResult};
use base::tokio_util::sync::CancellationToken;
use gmv_domain::info::media_info_ext::MediaExt;
use gmv_protocol::common::v1::ErrorDetail;
use log::{debug, info, warn};
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVERROR, AVERROR_EOF, AVERROR_EXIT, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_VIDEO, EAGAIN, av_rescale_q,
};
use rsmpeg::ffi::{AVMediaType, AVPacket};
use std::collections::VecDeque;
use std::ffi::c_int;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod codec;
pub mod event;
mod filter;
pub mod format;
pub mod utils;

/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步
const FIX_MAX_READ_FRAME: usize = 128;
const TRACK_DISCOVERY_MAX_DURATION: Duration = Duration::from_secs(2);
const TOPOLOGY_SETTLE_PACKETS: usize = 8;
const TOPOLOGY_SETTLE_BYTES: usize = 8 * 1024;
const AAC_MIME_CODEC: &str = "mp4a.40.2";

fn initial_track_window_complete(packet_count: usize, elapsed: Duration) -> bool {
    packet_count >= FIX_MAX_READ_FRAME || elapsed >= TRACK_DISCOVERY_MAX_DURATION
}

fn usable_video_keyframe(is_keyframe: bool, parameters_ready: bool) -> bool {
    is_keyframe && parameters_ready
}

fn initial_probe_can_finish(
    media_start_ready: bool,
    discovery_complete: bool,
    supported_params_ready: bool,
    topology_stable: bool,
) -> bool {
    media_start_ready && (discovery_complete || (supported_params_ready && topology_stable))
}

fn initial_audio_output_available(
    ready_audio_present: bool,
    rejected_audio_stream: bool,
    codec_has_output_audio: bool,
) -> bool {
    (ready_audio_present && !rejected_audio_stream) || codec_has_output_audio
}

fn passthrough_audio_is_stalled(last_audio_us: Option<i64>, master_clock_us: i64) -> bool {
    last_audio_us.is_some_and(|last| master_clock_us.saturating_sub(last) > AUDIO_STALL_GRACE_US)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCompletion {
    Eof,
    InputClosed,
    Cancelled,
}

#[derive(Debug)]
pub enum MediaRunError {
    Ffmpeg {
        stage: &'static str,
        code: c_int,
        message: String,
    },
    Interrupted(RtpInterruptReason),
    Pipeline(GlobalError),
}

impl From<GlobalError> for MediaRunError {
    fn from(error: GlobalError) -> Self {
        Self::Pipeline(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadFrameState {
    Packet,
    WouldBlock,
    Eof,
}

fn classify_read_frame(
    ret: c_int,
    stage: &'static str,
    read_control: &RtpReadControl,
) -> Result<ReadFrameState, MediaRunError> {
    if ret >= 0 {
        Ok(ReadFrameState::Packet)
    } else if ret == AVERROR(EAGAIN) {
        Ok(ReadFrameState::WouldBlock)
    } else if ret == AVERROR_EOF {
        Ok(ReadFrameState::Eof)
    } else if ret == AVERROR_EXIT {
        match read_control.interrupt_reason() {
            Some(reason) => Err(MediaRunError::Interrupted(reason)),
            None => Err(MediaRunError::Ffmpeg {
                stage,
                code: ret,
                message: show_ffmpeg_error_msg(ret),
            }),
        }
    } else {
        Err(MediaRunError::Ffmpeg {
            stage,
            code: ret,
            message: show_ffmpeg_error_msg(ret),
        })
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
    actual_media_profile: Option<ActualMediaProfile>,
    muxer_layer: Option<MuxerLayer>,
    pending_audio_generation: bool,
    output_generation: u64,
    passthrough_audio_last_us: Option<i64>,
    passthrough_audio_stalled: bool,
}
//idr帧及以后开始缓存
struct InitCacheInfo {
    //(rtp_ts累计,duration_ticks,pkt)
    pkts: VecDeque<AVPacket>,
    timeline_normalizer: TimelineNormalizer,
    ready_at_discovery: Vec<bool>,
    audio_observed_at_discovery: bool,
    discovery_snapshot_taken: bool,
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

unsafe fn repair_stream_parameters_if_needed(
    stream: *mut rsmpeg::ffi::AVStream,
    pkt: &AVPacket,
    media_ext: &MediaExt,
    param: &mut ParamRepairState,
) {
    let codecpar = unsafe { (*stream).codecpar };
    let audio_still_ready = !codecpar.is_null()
        && unsafe { (*codecpar).codec_type } == AVMediaType_AVMEDIA_TYPE_AUDIO
        && unsafe { audio_parameters_ready(codecpar) };
    if !param.ready
        || (!codecpar.is_null()
            && unsafe { (*codecpar).codec_type } == AVMediaType_AVMEDIA_TYPE_AUDIO
            && !audio_still_ready)
    {
        param.ready = unsafe { repair_basic_stream_info(stream, pkt, media_ext, param) };
    }
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

    pub fn init(
        ssrc: u32,
        stream_config: StreamConfig,
        cancel: CancellationToken,
    ) -> GlobalResult<(MediaContext, MuxerLayer)> {
        let read_control = Arc::new(RtpReadControl::new(
            cancel,
            stream_config.startup_io_deadline,
        ));
        let rtp_buffer = RtpPacketBuffer::init(
            ssrc,
            stream_config.rtp_rx,
            &stream_config.media_ext,
            read_control.clone(),
        );
        let demuxer_context = DemuxerContext::start_demuxer(
            ssrc,
            &stream_config.media_ext,
            rtp_buffer,
            read_control,
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
            actual_media_profile: None,
            muxer_layer: None,
            pending_audio_generation: false,
            output_generation: 1,
            passthrough_audio_last_us: None,
            passthrough_audio_stalled: false,
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
            ready_at_discovery: Vec::new(),
            audio_observed_at_discovery: false,
            discovery_snapshot_taken: false,
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
        let mut discovery_started_at = None;
        let mut discovery_complete = false;
        let mut read_backoff = Duration::from_millis(5);
        loop {
            let mut pkt = std::mem::zeroed::<AVPacket>();
            let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
            match classify_read_frame(
                ret,
                "fix_basic_stream_info",
                &self.demuxer_context.read_control,
            )? {
                ReadFrameState::Packet => {
                    read_backoff = Duration::from_millis(5);
                    self.demuxer_context.read_control.mark_startup_complete();
                    discovery_started_at.get_or_insert_with(Instant::now);
                }
                ReadFrameState::WouldBlock => {
                    if discovery_started_at.is_some_and(|started_at| {
                        started_at.elapsed() >= TRACK_DISCOVERY_MAX_DURATION
                    }) {
                        discovery_complete = true;
                        Self::snapshot_initial_tracks(&mut cache_info, &self.demuxer_context);
                        if !cache_info.pkts.is_empty() {
                            break;
                        }
                    }
                    std::thread::sleep(read_backoff);
                    read_backoff = (read_backoff * 2).min(Duration::from_millis(20));
                    continue;
                }
                ReadFrameState::Eof => break,
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
            let param = &mut self.demuxer_context.params[idx];
            let is_video = (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO;
            if is_video {
                repair_stream_parameters_if_needed(st, &pkt, ext, param);
            }
            if !repair_missing_timestamps(&mut pkt, (*codecpar).video_delay) {
                warn!("Discard packet without pts/dts; ssrc: {}", self.ssrc);
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                continue;
            }
            if !is_video {
                repair_stream_parameters_if_needed(st, &pkt, ext, param);
            }
            let parameters_ready = param.ready;
            // 标记状态
            match (*codecpar).codec_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    if usable_video_keyframe(
                        pkt.flags & AV_PKT_FLAG_KEY as i32 != 0,
                        parameters_ready,
                    ) {
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
            if !discovery_complete
                && discovery_started_at.is_some_and(|started_at| {
                    initial_track_window_complete(counter, started_at.elapsed())
                })
            {
                discovery_complete = true;
                Self::snapshot_initial_tracks(&mut cache_info, &self.demuxer_context);
            }
            if initial_probe_can_finish(
                should_cache,
                discovery_complete,
                supported_params_ready,
                topology_settle.is_stable(),
            ) {
                Self::snapshot_initial_tracks(&mut cache_info, &self.demuxer_context);
                break;
            }
        }

        Self::snapshot_initial_tracks(&mut cache_info, &self.demuxer_context);

        if cache_info.timeline_normalizer.global_base_us == i64::MAX {
            cache_info.timeline_normalizer.global_base_us = 0
        };
        dump_stream_info(&self.demuxer_context);

        Ok(cache_info)
    }

    fn snapshot_initial_tracks(cache: &mut InitCacheInfo, demuxer: &DemuxerContext) {
        if cache.discovery_snapshot_taken {
            return;
        }
        cache.ready_at_discovery = demuxer.params.iter().map(|param| param.ready).collect();
        cache.discovery_snapshot_taken = true;
        unsafe {
            cache.audio_observed_at_discovery = (0..demuxer.params.len()).any(|index| {
                let stream = *(*demuxer.avio.fmt_ctx).streams.add(index);
                !stream.is_null()
                    && !(*stream).codecpar.is_null()
                    && (*(*stream).codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO
            });
        }
    }

    fn set_audio_runtime(
        &self,
        source_state: AudioSourceRuntimeState,
        output_mode: OutputAudioRuntimeMode,
        recovery_eligible: bool,
        late_track_watch: bool,
    ) {
        let Some(stream_id) = self.stream_id.as_deref() else {
            return;
        };
        let (sample_rate, channels) = self.output_audio_parameters(output_mode);
        if !Register::try_set_audio_runtime(
            stream_id,
            AudioRuntimeMetadata {
                source_state,
                output_mode,
                recovery_eligible,
                late_track_watch,
                sample_rate,
                channels,
                generation: self.output_generation,
            },
        ) {
            debug!(
                "audio runtime update ignored: action=audio_runtime, outcome=ignored, reason=stream_finalized, stream_id={stream_id}"
            );
        }
    }

    fn output_audio_parameters(&self, output_mode: OutputAudioRuntimeMode) -> (u32, u32) {
        if output_mode == OutputAudioRuntimeMode::None {
            return (0, 0);
        }
        let Some(track) = self
            .demuxer_context
            .output_plan
            .tracks
            .iter()
            .find(|track| track.media_type == AVMediaType_AVMEDIA_TYPE_AUDIO)
        else {
            return (0, 0);
        };
        match track.source {
            OutputTrackSource::TranscodedAac(_) | OutputTrackSource::SilentAac => (48_000, 1),
            OutputTrackSource::Input(index) => unsafe {
                let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
                if fmt_ctx.is_null() || index >= (*fmt_ctx).nb_streams as usize {
                    return (0, 0);
                }
                let stream = *(*fmt_ctx).streams.add(index);
                if stream.is_null() || (*stream).codecpar.is_null() {
                    return (0, 0);
                }
                let codecpar = (*stream).codecpar;
                (
                    (*codecpar).sample_rate.max(0) as u32,
                    (*codecpar).ch_layout.nb_channels.max(0) as u32,
                )
            },
        }
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
            let source_audio_observed = cache_info.audio_observed_at_discovery;
            let audio_expected =
                self.media_ext.declaration.audio.is_active() || source_audio_observed;
            let ready_audio_present =
                cache_info
                    .ready_at_discovery
                    .iter()
                    .enumerate()
                    .any(|(index, ready)| {
                        if !*ready {
                            return false;
                        }
                        let stream = *(*self.demuxer_context.avio.fmt_ctx).streams.add(index);
                        !stream.is_null()
                            && !(*stream).codecpar.is_null()
                            && (*(*stream).codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO
                    });
            if audio_expected && !ready_audio_present && self.codec_context.is_none() {
                self.codec_context = Some(CodecContext::fixed_aac());
            }
            if let Some(codec) = &mut self.codec_context {
                codec.prepare(
                    &mut self.demuxer_context,
                    audio_expected,
                    &cache_info.ready_at_discovery,
                )?;
            }
            let rejected_audio_stream = self
                .codec_context
                .as_ref()
                .and_then(CodecContext::rejected_audio_stream);
            let source_audio_usable = ready_audio_present && rejected_audio_stream.is_none();
            let output_audio_available = initial_audio_output_available(
                ready_audio_present,
                rejected_audio_stream.is_some(),
                self.codec_context
                    .as_ref()
                    .is_some_and(CodecContext::has_output_audio),
            );
            let planned_audio_expected = audio_expected && output_audio_available;
            let mut output_ready_at_discovery = cache_info.ready_at_discovery.clone();
            if let Some(index) = rejected_audio_stream {
                if let Some(ready) = output_ready_at_discovery.get_mut(index as usize) {
                    *ready = false;
                }
            }
            self.demuxer_context
                .freeze_output_plan(planned_audio_expected, &output_ready_at_discovery);
            if let Some(index) = self
                .codec_context
                .as_ref()
                .and_then(CodecContext::transcoded_stream_index)
            {
                self.demuxer_context
                    .output_plan
                    .mark_transcoded_audio(index);
            }
            let output_audio_mode = if self.demuxer_context.output_plan.has_silent_audio() {
                OutputAudioRuntimeMode::SilentPlaceholder
            } else if source_audio_usable {
                OutputAudioRuntimeMode::Real
            } else {
                OutputAudioRuntimeMode::None
            };
            let source_audio_state = if rejected_audio_stream.is_some() {
                AudioSourceRuntimeState::Failed
            } else if source_audio_usable {
                AudioSourceRuntimeState::Ready
            } else if audio_expected && !output_audio_available {
                AudioSourceRuntimeState::Failed
            } else if source_audio_observed {
                AudioSourceRuntimeState::DetectedUnready
            } else if self.media_ext.declaration.audio.is_active() {
                AudioSourceRuntimeState::DeclaredUnobserved
            } else {
                AudioSourceRuntimeState::NotExpected
            };
            let initial_recovery_eligible = match output_audio_mode {
                OutputAudioRuntimeMode::SilentPlaceholder => true,
                OutputAudioRuntimeMode::Real => self
                    .codec_context
                    .as_ref()
                    .is_none_or(|codec| !codec.has_real_audio() || codec.has_silent_audio()),
                OutputAudioRuntimeMode::None => false,
            };
            self.set_audio_runtime(
                source_audio_state,
                output_audio_mode,
                initial_recovery_eligible,
                true,
            );
            let media_param = extradata::parse_media_param(&self.demuxer_context);
            let mut actual_media_profile = ActualMediaProfile {
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
            if self.demuxer_context.output_plan.has_fixed_aac() {
                actual_media_profile.audio_codec = AAC_MIME_CODEC.to_string();
            }
            if let Some(stream_id) = self.stream_id.as_deref() {
                if !Register::try_set_actual_media_profile(stream_id, actual_media_profile.clone())
                {
                    return Ok(MediaCompletion::InputClosed);
                }
            }
            self.actual_media_profile = Some(actual_media_profile);
            self.muxer_layer = Some(muxer_layer.clone());
            //初始化muxer
            let (muxer_context, muxer_failures) =
                MuxerContext::init_collect(&self.demuxer_context, muxer_layer);
            self.muxer_context = muxer_context;
            for (muxer, error) in muxer_failures {
                self.fail_muxer(muxer, "output_muxer_failed", error)?;
            }
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
            let mut read_backoff = Duration::from_millis(5);

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
                match classify_read_frame(ret, "read_frame", &self.demuxer_context.read_control)? {
                    ReadFrameState::Packet => read_backoff = Duration::from_millis(5),
                    ReadFrameState::WouldBlock => {
                        std::thread::sleep(read_backoff);
                        read_backoff = (read_backoff * 2).min(Duration::from_millis(20));
                        continue;
                    }
                    ReadFrameState::Eof => break,
                }
                self.observe_packet_track(&pkt, &mut normalizer)?;
                if pkt.stream_index < 0
                    || pkt.stream_index as usize >= self.demuxer_context.params.len()
                {
                    rsmpeg::ffi::av_packet_unref(&mut pkt);
                    continue;
                }

                let process_result = self.process(&mut normalizer, &mut pkt);
                rsmpeg::ffi::av_packet_unref(&mut pkt);
                process_result?;
            }
            //write end
            self.finish_pipeline()?;
        }

        Ok(MediaCompletion::Eof)
    }

    fn finish_pipeline(&mut self) -> GlobalResult<()> {
        let mut audio_flush_failed = false;
        let packets = match &mut self.codec_context {
            Some(codec) => match codec.flush() {
                Ok(packets) => packets,
                Err(_) => {
                    audio_flush_failed = true;
                    codec.degrade_to_silence();
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        if audio_flush_failed {
            warn!(
                "audio track degraded: stage=flush, outcome=video_continues, stream_id={}, ssrc={}",
                self.stream_id.as_deref().unwrap_or("unknown"),
                self.ssrc
            );
            self.set_audio_runtime(
                AudioSourceRuntimeState::Failed,
                if self
                    .codec_context
                    .as_ref()
                    .is_some_and(CodecContext::has_silent_audio)
                {
                    OutputAudioRuntimeMode::SilentPlaceholder
                } else {
                    OutputAudioRuntimeMode::None
                },
                self.codec_context
                    .as_ref()
                    .is_some_and(CodecContext::has_silent_audio),
                false,
            );
        }
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
            let media_type = if pkt.stream_index >= 0 {
                let stream = *(*self.demuxer_context.avio.fmt_ctx)
                    .streams
                    .add(pkt.stream_index as usize);
                if !stream.is_null() && !(*stream).codecpar.is_null() {
                    Some((*(*stream).codecpar).codec_type)
                } else {
                    None
                }
            } else {
                None
            };
            let is_video = media_type == Some(AVMediaType_AVMEDIA_TYPE_VIDEO);
            let codec_handles_packet = self
                .codec_context
                .as_ref()
                .is_some_and(|codec| codec.handles(pkt));
            if media_type == Some(AVMediaType_AVMEDIA_TYPE_AUDIO)
                && !codec_handles_packet
                && self
                    .demuxer_context
                    .output_plan
                    .contains_packet_index(pkt.stream_index)
            {
                self.passthrough_audio_last_us = Some(master_clock_us);
                if self.passthrough_audio_stalled {
                    self.passthrough_audio_stalled = false;
                    self.set_audio_runtime(
                        AudioSourceRuntimeState::Ready,
                        OutputAudioRuntimeMode::Real,
                        true,
                        true,
                    );
                }
            }
            // 调用 muxer 其中master_clock_us需要转换为秒，供录制进度信息
            if is_video && pkt.flags & AV_PKT_FLAG_KEY as i32 != 0 && self.pending_audio_generation
            {
                self.activate_late_audio_generation()?;
            }
            if codec_handles_packet {
                self.codec_context
                    .as_mut()
                    .expect("codec context checked")
                    .note_real_audio(master_clock_us);
                let packets = self
                    .codec_context
                    .as_mut()
                    .expect("codec context checked")
                    .process(pkt);
                match packets {
                    Ok(packets) => {
                        for packet in packets {
                            Self::handle_pkt_muxer(
                                self,
                                res,
                                &packet,
                                (packet.pts.max(0) as u64) / 48_000,
                            )?;
                        }
                    }
                    Err(_) => {
                        warn!(
                            "audio track degraded: stage=normalize, outcome=silent_placeholder, stream_id={}, ssrc={}",
                            self.stream_id.as_deref().unwrap_or("unknown"),
                            self.ssrc
                        );
                        self.codec_context
                            .as_mut()
                            .expect("codec context checked")
                            .degrade_to_silence();
                        let silent_available = self
                            .codec_context
                            .as_ref()
                            .is_some_and(CodecContext::has_silent_audio);
                        self.set_audio_runtime(
                            AudioSourceRuntimeState::Failed,
                            if silent_available {
                                OutputAudioRuntimeMode::SilentPlaceholder
                            } else {
                                OutputAudioRuntimeMode::None
                            },
                            silent_available,
                            silent_available,
                        );
                    }
                }
            } else if self
                .demuxer_context
                .output_plan
                .contains_packet_index(pkt.stream_index)
            {
                Self::handle_pkt_muxer(self, res, pkt, (master_clock_us / 1_000_000) as u64)?;
            }
            if is_video {
                if !self.passthrough_audio_stalled
                    && passthrough_audio_is_stalled(self.passthrough_audio_last_us, master_clock_us)
                {
                    self.passthrough_audio_stalled = true;
                    self.set_audio_runtime(
                        AudioSourceRuntimeState::Unavailable,
                        OutputAudioRuntimeMode::Real,
                        true,
                        true,
                    );
                }
                let mut silent_audio_failed = false;
                let real_audio_before = self
                    .codec_context
                    .as_ref()
                    .is_some_and(CodecContext::has_real_audio);
                let silent_packets = match self.codec_context.as_mut() {
                    Some(codec) => match codec.silence_until(master_clock_us) {
                        Ok(packets) => packets,
                        Err(_) => {
                            warn!(
                                "audio track degraded: stage=silent_aac, outcome=audio_disabled, stream_id={}, ssrc={}",
                                self.stream_id.as_deref().unwrap_or("unknown"),
                                self.ssrc
                            );
                            codec.disable_audio();
                            silent_audio_failed = true;
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                if real_audio_before
                    && self
                        .codec_context
                        .as_ref()
                        .is_some_and(|codec| !codec.has_real_audio())
                {
                    let silent_available = self
                        .codec_context
                        .as_ref()
                        .is_some_and(CodecContext::has_silent_audio);
                    self.set_audio_runtime(
                        AudioSourceRuntimeState::Unavailable,
                        if silent_available {
                            OutputAudioRuntimeMode::SilentPlaceholder
                        } else {
                            OutputAudioRuntimeMode::None
                        },
                        silent_available,
                        silent_available,
                    );
                } else if silent_audio_failed {
                    self.set_audio_runtime(
                        AudioSourceRuntimeState::Failed,
                        OutputAudioRuntimeMode::None,
                        false,
                        false,
                    );
                }
                for packet in silent_packets {
                    Self::handle_pkt_muxer(
                        self,
                        res,
                        &packet,
                        (packet.pts.max(0) as u64) / 48_000,
                    )?;
                }
            }
        }
        Ok(())
    }

    unsafe fn observe_packet_track(
        &mut self,
        pkt: &AVPacket,
        normalizer: &mut TimelineNormalizer,
    ) -> GlobalResult<()> {
        unsafe {
            sync_discovered_streams(&mut self.demuxer_context, normalizer);
            if pkt.stream_index < 0
                || pkt.stream_index as usize >= self.demuxer_context.params.len()
            {
                return Ok(());
            }
            let index = pkt.stream_index as usize;
            let stream = *(*self.demuxer_context.avio.fmt_ctx).streams.add(index);
            if stream.is_null() || (*stream).codecpar.is_null() {
                return Ok(());
            }
            if pkt.size > 0 && !pkt.data.is_null() {
                repair_stream_parameters_if_needed(
                    stream,
                    pkt,
                    &self.media_ext,
                    &mut self.demuxer_context.params[index],
                );
            }
            if self.demuxer_context.params[index].ready
                && (*(*stream).codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO
            {
                let output_has_audio = self.demuxer_context.output_plan.has_audio();
                if self.codec_context.is_none() {
                    self.codec_context = Some(CodecContext::fixed_aac());
                }
                if !output_has_audio {
                    let placeholder_result = self
                        .codec_context
                        .as_mut()
                        .expect("codec context initialized")
                        .enable_late_placeholder();
                    if placeholder_result.is_err() {
                        self.set_audio_runtime(
                            AudioSourceRuntimeState::Failed,
                            OutputAudioRuntimeMode::None,
                            false,
                            false,
                        );
                        return Ok(());
                    }
                }
                let recovered_output_index = if self.demuxer_context.output_plan.has_silent_audio()
                {
                    SYNTHETIC_AUDIO_PACKET_INDEX
                } else {
                    pkt.stream_index
                };
                let mut activation_error = None;
                let activated = match self.codec_context.as_mut() {
                    Some(codec) => match codec.observe_ready_audio(
                        &mut self.demuxer_context,
                        pkt.stream_index,
                        recovered_output_index,
                    ) {
                        Ok(activated) => activated,
                        Err(error) => {
                            codec.degrade_to_silence();
                            activation_error = Some(error);
                            false
                        }
                    },
                    None => false,
                };
                if let Some(error) = activation_error {
                    self.set_audio_runtime(
                        AudioSourceRuntimeState::Failed,
                        OutputAudioRuntimeMode::SilentPlaceholder,
                        true,
                        true,
                    );
                    warn!(
                        "late audio activation failed: action=audio_recovery, outcome=silent_placeholder, stream_id={}, ssrc={}, reason={error}",
                        self.stream_id.as_deref().unwrap_or("unknown"),
                        self.ssrc
                    );
                }
                if activated {
                    if output_has_audio {
                        self.set_audio_runtime(
                            AudioSourceRuntimeState::Ready,
                            OutputAudioRuntimeMode::Real,
                            true,
                            true,
                        );
                    } else {
                        self.pending_audio_generation = true;
                        self.set_audio_runtime(
                            AudioSourceRuntimeState::DetectedUnready,
                            OutputAudioRuntimeMode::None,
                            true,
                            true,
                        );
                    }
                }
            }
            Ok(())
        }
    }

    fn activate_late_audio_generation(&mut self) -> GlobalResult<()> {
        let Some(muxer_layer) = self.muxer_layer.clone() else {
            return Ok(());
        };
        let previous_plan = self.demuxer_context.output_plan.clone();
        self.demuxer_context.output_plan.add_silent_audio();
        match MuxerContext::init(&self.demuxer_context, muxer_layer) {
            Ok(new_muxer) => {
                let mut old_muxer = std::mem::replace(&mut self.muxer_context, new_muxer);
                Self::handle_pkt_muxer_end(&mut old_muxer);
                self.output_generation = self.output_generation.saturating_add(1);
                self.pending_audio_generation = false;
                if let Some(profile) = self.actual_media_profile.as_mut() {
                    profile.audio_codec = AAC_MIME_CODEC.to_string();
                    if let Some(stream_id) = self.stream_id.as_deref() {
                        if !Register::try_set_actual_media_profile(stream_id, profile.clone()) {
                            debug!(
                                "media profile update ignored: action=media_profile, outcome=ignored, reason=stream_finalized, stream_id={stream_id}"
                            );
                        }
                    }
                }
                self.set_audio_runtime(
                    AudioSourceRuntimeState::Ready,
                    OutputAudioRuntimeMode::Real,
                    true,
                    true,
                );
                self.sync_output_readiness()?;
                debug!(
                    "media output generation replaced: stage=late_audio, outcome=ready, stream_id={}, ssrc={}, generation={}",
                    self.stream_id.as_deref().unwrap_or("unknown"),
                    self.ssrc,
                    self.output_generation
                );
            }
            Err(error) => {
                self.demuxer_context.output_plan = previous_plan;
                self.pending_audio_generation = false;
                if let Some(codec) = self.codec_context.as_mut() {
                    codec.disable_audio();
                }
                self.set_audio_runtime(
                    AudioSourceRuntimeState::Failed,
                    OutputAudioRuntimeMode::None,
                    false,
                    false,
                );
                warn!(
                    "late audio generation rejected: stage=muxer_init, outcome=video_continues, stream_id={}, ssrc={}, reason={error}",
                    self.stream_id.as_deref().unwrap_or("unknown"),
                    self.ssrc
                );
            }
        }
        Ok(())
    }

    fn handle_pkt_muxer(
        &mut self,
        epoch: ProcessResult,
        pkt: &AVPacket,
        ts: u64,
    ) -> GlobalResult<()> {
        let flv_error = if let Some(context) = &mut self.muxer_context.flv {
            match context {
                FlvSupperCtx::FlvCtx(context) => context.write_packet(pkt, ts).err(),
                FlvSupperCtx::H265FlvCtx(context) => context.write_packet(pkt, ts).err(),
            }
        } else {
            None
        };
        if let Some(error) = flv_error {
            self.fail_muxer(MuxerEnum::Flv, "output_muxer_failed", error)?;
        }
        let mp4_error = self
            .muxer_context
            .mp4
            .as_mut()
            .and_then(|context| context.write_packet(pkt, ts).err());
        if let Some(error) = mp4_error {
            self.fail_muxer(MuxerEnum::Mp4, "output_muxer_failed", error)?;
        }
        if self.muxer_context.ts.is_some() {
            warn!("stream packet mux ignored unsupported ts output");
        }
        if self.muxer_context.rtp_frame.is_some() {
            warn!("stream packet mux ignored unsupported rtp-frame output");
        }
        if self.muxer_context.rtp_ps.is_some() {
            warn!("stream packet mux ignored unsupported rtp-ps output");
        }
        if self.muxer_context.rtp_enc.is_some() {
            warn!("stream packet mux ignored unsupported rtp-enc output");
        }
        if self.muxer_context.hls_ts.is_some() {
            warn!("stream packet mux ignored unsupported hls-ts output");
        }
        let fmp4_error = if let Some(context) = &mut self.muxer_context.fmp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            context.write_packet(pkt, ts).err()
        } else {
            None
        };
        if let Some(error) = fmp4_error {
            self.fail_muxer(MuxerEnum::FMp4, "output_muxer_failed", error)?;
        }
        let dash_error = if let Some(context) = &mut self.muxer_context.dash_mp4 {
            if epoch == ProcessResult::Discontinuity {
                context.epoch = Instant::now();
            }
            context.write_packet(pkt, ts).err()
        } else {
            None
        };
        if let Some(error) = dash_error {
            self.fail_muxer(MuxerEnum::DashMp4, "output_muxer_failed", error)?;
        }
        if epoch == ProcessResult::Discontinuity {
            if let Some(mut context) = self.muxer_context.hls_mp4.take() {
                let pkt_tx = context.pkt_tx.clone();
                context.flush();
                let next_segment_seq = context.next_segment_seq();
                match HlsFmp4Context::init_context(&self.demuxer_context, pkt_tx) {
                    Ok(mut context) => {
                        context.set_segment_seq(next_segment_seq);
                        self.muxer_context.hls_mp4 = Some(context);
                    }
                    Err(error) => {
                        self.fail_muxer(MuxerEnum::HlsMp4, "output_muxer_failed", error)?;
                    }
                }
            }
        }
        let hls_error = self
            .muxer_context
            .hls_mp4
            .as_mut()
            .and_then(|context| context.write_packet(pkt, ts).err());
        if let Some(error) = hls_error {
            self.fail_muxer(MuxerEnum::HlsMp4, "output_muxer_failed", error)?;
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
                let muxer = m_event.muxer();
                if let Err(error) =
                    m_event.handle_event(&mut self.muxer_context, &self.demuxer_context)
                {
                    self.fail_muxer(muxer, "output_muxer_failed", error)?;
                }
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

    fn fail_muxer(
        &mut self,
        muxer: MuxerEnum,
        code: &str,
        error_value: GlobalError,
    ) -> GlobalResult<()> {
        match muxer {
            MuxerEnum::Flv => self.muxer_context.flv = None,
            MuxerEnum::Mp4 => self.muxer_context.mp4 = None,
            MuxerEnum::Ts => self.muxer_context.ts = None,
            MuxerEnum::FMp4 => self.muxer_context.fmp4 = None,
            MuxerEnum::HlsMp4 => self.muxer_context.hls_mp4 = None,
            MuxerEnum::DashMp4 => self.muxer_context.dash_mp4 = None,
            MuxerEnum::HlsTs => self.muxer_context.hls_ts = None,
            MuxerEnum::RtpFrame => self.muxer_context.rtp_frame = None,
            MuxerEnum::RtpPs => self.muxer_context.rtp_ps = None,
            MuxerEnum::RtpEnc => self.muxer_context.rtp_enc = None,
        }
        let Some(stream_id) = self.stream_id.as_deref() else {
            warn!(
                "stream output failed without stream identity: muxer={muxer:?}, error={error_value}"
            );
            return Ok(());
        };
        let mut metadata = std::collections::HashMap::new();
        if let Some(profile) = self.actual_media_profile.as_ref() {
            metadata.insert("video_codec".to_string(), profile.video_codec.clone());
            metadata.insert("audio_codec".to_string(), profile.audio_codec.clone());
        }
        let output_type = match muxer {
            MuxerEnum::Flv => "flv",
            MuxerEnum::Mp4 => "mp4",
            MuxerEnum::FMp4 => "fmp4",
            MuxerEnum::HlsMp4 => "hls|ll_hls",
            MuxerEnum::DashMp4 => "dash_mp4",
            MuxerEnum::Ts => "ts",
            MuxerEnum::HlsTs => "hls_ts",
            MuxerEnum::RtpFrame => "rtp_frame",
            MuxerEnum::RtpPs => "rtp_ps",
            MuxerEnum::RtpEnc => "rtp_enc",
        };
        warn!(
            "stream output failed: action=output_mux, outcome=failed, scope=output_only, other_outputs=unaffected, stream_id={stream_id}, ssrc={}, output_type={output_type}, muxer={muxer:?}, code={code}, error={error_value}",
            self.ssrc
        );
        Register::mark_muxer_failed(
            stream_id,
            muxer,
            ErrorDetail {
                code: code.to_string(),
                message: error_value.to_string(),
                metadata,
            },
        )
    }
}

fn set_output_ready(
    stream_id: &str,
    output_type: &str,
    profile: &ActualMediaProfile,
) -> GlobalResult<()> {
    let mime_codec = output_mime_codec(output_type, profile);
    let next_metadata = OutputMediaMetadata {
        state: OutputRuntimeState::Ready,
        video_codec: profile.video_codec.clone(),
        audio_codec: profile.audio_codec.clone(),
        mime_codec: mime_codec.clone(),
        failure: None,
    };
    if Register::output_media_metadata(stream_id, output_type).as_ref() == Some(&next_metadata) {
        return Ok(());
    }
    let updated = Register::try_set_output_media_metadata(stream_id, output_type, next_metadata)?;
    if !updated {
        debug!(
            "output ready state update ignored: action=output_metadata, outcome=ignored, reason=stream_finalized, stream_id={stream_id}, output_type={output_type}"
        );
    } else {
        info!(
            "stream output ready: action=output_metadata, stage=output_ready, outcome=ready, stream_id={stream_id}, output_type={output_type}, video_codec={}, audio_codec={}, mime_codec={mime_codec}",
            profile.video_codec, profile.audio_codec
        );
    }
    Ok(())
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
    use base::tokio_util::sync::CancellationToken;
    use rsmpeg::ffi::{
        AV_NOPTS_VALUE, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264,
        AVMediaType_AVMEDIA_TYPE_DATA, av_malloc, avformat_alloc_context, avformat_free_context,
        avformat_new_stream,
    };

    #[test]
    fn read_frame_classifies_packet_eof_and_failure() {
        let control = RtpReadControl::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(
            classify_read_frame(0, "test", &control).unwrap(),
            ReadFrameState::Packet
        );
        assert_eq!(
            classify_read_frame(AVERROR(EAGAIN), "test", &control).unwrap(),
            ReadFrameState::WouldBlock
        );
        assert_eq!(
            classify_read_frame(AVERROR_EOF, "test", &control).unwrap(),
            ReadFrameState::Eof
        );

        match classify_read_frame(-1, "demux", &control) {
            Err(MediaRunError::Ffmpeg { stage, code, .. }) => {
                assert_eq!(stage, "demux");
                assert_eq!(code, -1);
            }
            _ => panic!("non-EOF FFmpeg result must remain a failure"),
        }
    }

    #[test]
    fn initial_track_window_is_bounded_by_packets_or_two_seconds() {
        assert!(!initial_track_window_complete(
            FIX_MAX_READ_FRAME - 1,
            TRACK_DISCOVERY_MAX_DURATION - Duration::from_millis(1),
        ));
        assert!(initial_track_window_complete(
            FIX_MAX_READ_FRAME,
            Duration::ZERO,
        ));
        assert!(initial_track_window_complete(
            1,
            TRACK_DISCOVERY_MAX_DURATION,
        ));
    }

    #[test]
    fn track_window_does_not_publish_video_before_parameters_and_keyframe_are_ready() {
        assert!(!usable_video_keyframe(true, false));
        assert!(!initial_probe_can_finish(false, true, false, false));

        assert!(usable_video_keyframe(true, true));
        assert!(initial_probe_can_finish(true, true, false, false));
    }

    #[test]
    fn video_parameters_are_collected_before_missing_timestamps_reject_packet() {
        unsafe {
            let format = avformat_alloc_context();
            assert!(!format.is_null());
            let stream = avformat_new_stream(format, std::ptr::null());
            assert!(!stream.is_null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_VIDEO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_H264;

            let mut annex_b = vec![
                0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0x01, 0xe0, 0x08, 0x9f, 0x97, 0x01, 0x6e,
                0x40, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
            ];
            let mut packet = std::mem::zeroed::<AVPacket>();
            packet.data = annex_b.as_mut_ptr();
            packet.size = annex_b.len() as i32;
            packet.pts = AV_NOPTS_VALUE;
            packet.dts = AV_NOPTS_VALUE;
            let mut state = ParamRepairState::default();

            repair_stream_parameters_if_needed(stream, &packet, &MediaExt::default(), &mut state);

            let parameter_sets = state.h264_ps.as_ref().expect("H.264 evidence missing");
            assert!(parameter_sets.sps.is_some());
            assert!(parameter_sets.pps.is_some());
            assert!(!repair_missing_timestamps(&mut packet, 0));

            avformat_free_context(format);
        }
    }

    #[test]
    fn stale_audio_ready_state_is_revalidated_from_later_adts() {
        unsafe {
            let format = avformat_alloc_context();
            assert!(!format.is_null());
            let stream = avformat_new_stream(format, std::ptr::null());
            assert!(!stream.is_null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_AAC;
            (*codecpar).extradata = av_malloc(2) as *mut u8;
            (*codecpar).extradata_size = 2;
            std::ptr::copy_nonoverlapping([0x12, 0x00].as_ptr(), (*codecpar).extradata, 2);

            let mut adts = [0xff, 0xf1, 0x4c, 0x40, 0, 0, 0];
            let mut packet = std::mem::zeroed::<AVPacket>();
            packet.data = adts.as_mut_ptr();
            packet.size = adts.len() as i32;
            let mut state = ParamRepairState {
                ready: true,
                ..Default::default()
            };

            repair_stream_parameters_if_needed(stream, &packet, &MediaExt::default(), &mut state);

            assert!(state.ready);
            assert_eq!((*codecpar).sample_rate, 48_000);
            assert_eq!((*codecpar).ch_layout.nb_channels, 1);
            avformat_free_context(format);
        }
    }

    #[test]
    fn ready_aac_passthrough_is_available_without_codec_context() {
        assert!(initial_audio_output_available(true, false, false));
        assert!(!initial_audio_output_available(true, true, false));
        assert!(initial_audio_output_available(false, false, true));
    }

    #[test]
    fn passthrough_audio_stall_uses_shared_grace_period() {
        assert!(!passthrough_audio_is_stalled(
            Some(1_000_000),
            1_000_000 + AUDIO_STALL_GRACE_US,
        ));
        assert!(passthrough_audio_is_stalled(
            Some(1_000_000),
            1_000_001 + AUDIO_STALL_GRACE_US,
        ));
        assert!(!passthrough_audio_is_stalled(None, i64::MAX));
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
            audio_codec: AAC_MIME_CODEC.to_string(),
        };

        assert_eq!(
            output_mime_codec("fmp4", &profile),
            "video/mp4; codecs=\"hev1.1.6.L78, mp4a.40.2\""
        );
        assert_eq!(output_mime_codec("flv", &profile), "video/x-flv");
    }
}
