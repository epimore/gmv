use crate::media::{DEFAULT_IO_BUF_SIZE, rtp, rw, show_ffmpeg_error_msg};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use gmv_domain::info::media_info_ext::MediaExt;
use rsmpeg::ffi::{
    AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_ADPCM_G722,
    AVCodecID_AV_CODEC_ID_G723_1, AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H263,
    AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_MPEG4,
    AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW,
    AVCodecID_AV_CODEC_ID_SIREN, AVCodecParameters, AVDictionary, AVFMT_FLAG_CUSTOM_IO,
    AVFMT_FLAG_DISCARD_CORRUPT, AVFMT_FLAG_GENPTS, AVFMT_FLAG_IGNDTS, AVFMT_FLAG_IGNIDX,
    AVFMT_FLAG_NOBUFFER, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVRational, AVStream, av_channel_layout_default,
    av_channel_layout_uninit, av_dict_free, av_find_input_format, av_free, av_malloc,
    avcodec_find_decoder, avcodec_parameters_alloc, avcodec_parameters_copy,
    avcodec_parameters_free, avformat_alloc_context, avformat_close_input,
    avformat_find_stream_info, avformat_free_context, avformat_new_stream, avformat_open_input,
    avio_alloc_context, avio_context_free,
};
use std::ffi::{CString, c_int, c_void};
use std::ops::Range;
use std::ptr;
use std::sync::Arc;

type OpaquePtr = *mut rtp::RtpPacketBuffer;

/// Wrapper that owns FFmpeg resources for an input (fmt_ctx + avio_ctx + io_buf + opaque)
pub struct AvioResource {
    pub fmt_ctx: *mut AVFormatContext,
    /// raw buffer pointer passed to avio_alloc_context
    pub io_buf: *mut u8,
    pub avio_ctx: *mut AVIOContext,
}
// unsafe impl Send for AvioResource {} // only safe if you ensure no concurrent mutable use across threads

impl Drop for AvioResource {
    fn drop(&mut self) {
        unsafe {
            // 1) 读取并释放opaque（优先处理）
            let mut opaque_ptr = ptr::null_mut();
            if !self.avio_ctx.is_null() {
                opaque_ptr = (*self.avio_ctx).opaque;
                (*self.avio_ctx).opaque = ptr::null_mut(); // 解除关联
            }
            if !opaque_ptr.is_null() {
                let tup_ptr = opaque_ptr as OpaquePtr;
                drop(Box::from_raw(tup_ptr)); // 安全回收
            }
            // 2) 释放avio_ctx（内部会释放io_buf）
            if !self.avio_ctx.is_null() {
                let mut local = self.avio_ctx;
                avio_context_free(&mut local); // 自动释放io_buf
                self.avio_ctx = ptr::null_mut();
            }
            // 3) 关闭fmt_ctx
            if !self.fmt_ctx.is_null() {
                (*self.fmt_ctx).pb = ptr::null_mut();
                let mut local_fmt = self.fmt_ctx;
                avformat_close_input(&mut local_fmt);
                self.fmt_ctx = ptr::null_mut();
            }
            // 4) 移除手动释放io_buf的代码
            self.io_buf = ptr::null_mut(); // 无需手动释放
        }
    }
}
pub struct DemuxerContext {
    pub avio: AvioResource,
    /// we own `*mut AVCodecParameters` pointers and must free them in Drop
    pub params: Vec<ParamRepairState>,
    pub read_control: Arc<rtp::RtpReadControl>,
    pub output_plan: OutputTrackPlan,
}

pub const SYNTHETIC_AUDIO_PACKET_INDEX: i32 = i32::MAX - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputTrackSource {
    Input(usize),
    TranscodedAac(usize),
    SilentAac,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputTrack {
    pub packet_index: i32,
    pub source: OutputTrackSource,
    pub media_type: rsmpeg::ffi::AVMediaType,
}

#[derive(Clone, Debug, Default)]
pub struct OutputTrackPlan {
    pub tracks: Vec<OutputTrack>,
}

impl OutputTrackPlan {
    pub fn has_audio(&self) -> bool {
        self.tracks
            .iter()
            .any(|track| track.media_type == AVMediaType_AVMEDIA_TYPE_AUDIO)
    }

    pub fn has_silent_audio(&self) -> bool {
        self.tracks
            .iter()
            .any(|track| track.source == OutputTrackSource::SilentAac)
    }

    pub fn has_fixed_aac(&self) -> bool {
        self.tracks.iter().any(|track| {
            matches!(
                track.source,
                OutputTrackSource::TranscodedAac(_) | OutputTrackSource::SilentAac
            )
        })
    }

    pub fn mark_transcoded_audio(&mut self, input_index: usize) {
        if let Some(track) = self.tracks.iter_mut().find(|track| {
            track.media_type == AVMediaType_AVMEDIA_TYPE_AUDIO
                && track.source == OutputTrackSource::Input(input_index)
        }) {
            track.source = OutputTrackSource::TranscodedAac(input_index);
        }
    }

    pub fn contains_packet_index(&self, packet_index: i32) -> bool {
        self.tracks
            .iter()
            .any(|track| track.packet_index == packet_index)
    }

    pub fn add_silent_audio(&mut self) {
        if self.has_audio() {
            return;
        }
        self.tracks.push(OutputTrack {
            packet_index: SYNTHETIC_AUDIO_PACKET_INDEX,
            source: OutputTrackSource::SilentAac,
            media_type: AVMediaType_AVMEDIA_TYPE_AUDIO,
        });
    }
}

fn extend_param_repair_states(
    params: &mut Vec<ParamRepairState>,
    stream_count: usize,
) -> Range<usize> {
    let previous_count = params.len();
    if stream_count > previous_count {
        params.resize_with(stream_count, ParamRepairState::default);
    }
    previous_count..params.len()
}

fn ready_for_initial_output(
    media_type: rsmpeg::ffi::AVMediaType,
    index: usize,
    currently_ready: bool,
    ready_at_discovery: &[bool],
) -> bool {
    match media_type {
        AVMediaType_AVMEDIA_TYPE_VIDEO => index < ready_at_discovery.len() && currently_ready,
        AVMediaType_AVMEDIA_TYPE_AUDIO => ready_at_discovery.get(index).copied().unwrap_or(false),
        _ => false,
    }
}

impl DemuxerContext {
    pub(in crate::media::context) fn sync_params(&mut self) -> Range<usize> {
        let stream_count = unsafe { (*self.avio.fmt_ctx).nb_streams as usize };
        let added = extend_param_repair_states(&mut self.params, stream_count);
        for index in added.clone() {
            self.params[index] =
                unsafe { ParamRepairState::from_stream(*(*self.avio.fmt_ctx).streams.add(index)) };
        }
        added
    }

    pub(in crate::media::context) unsafe fn freeze_output_plan(
        &mut self,
        audio_expected: bool,
        ready_at_discovery: &[bool],
    ) {
        let fmt_ctx = self.avio.fmt_ctx;
        let mut video_selected = false;
        let mut audio_selected = false;
        let mut tracks = Vec::with_capacity(2);
        for index in 0..self.params.len() {
            let stream = unsafe { *(*fmt_ctx).streams.add(index) };
            if stream.is_null() || unsafe { (*stream).codecpar.is_null() } {
                continue;
            }
            let media_type = unsafe { (*(*stream).codecpar).codec_type };
            if !ready_for_initial_output(
                media_type,
                index,
                self.params[index].ready,
                ready_at_discovery,
            ) {
                continue;
            }
            let selected = match media_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO if !video_selected => {
                    video_selected = true;
                    true
                }
                AVMediaType_AVMEDIA_TYPE_AUDIO if !audio_selected => {
                    audio_selected = true;
                    true
                }
                _ => false,
            };
            if selected {
                tracks.push(OutputTrack {
                    packet_index: index as i32,
                    source: OutputTrackSource::Input(index),
                    media_type,
                });
            }
        }
        if audio_expected && !audio_selected {
            tracks.push(OutputTrack {
                packet_index: SYNTHETIC_AUDIO_PACKET_INDEX,
                source: OutputTrackSource::SilentAac,
                media_type: AVMediaType_AVMEDIA_TYPE_AUDIO,
            });
        }
        self.output_plan = OutputTrackPlan { tracks };
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ParameterEvidence {
    #[default]
    Unknown,
    ProtocolConstraint,
    Sdp,
    Demux,
    Bitstream,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackParameterEvidence {
    pub codec: ParameterEvidence,
    pub sample_rate: ParameterEvidence,
    pub channels: ParameterEvidence,
    pub extradata: ParameterEvidence,
    pub time_base: ParameterEvidence,
}

impl TrackParameterEvidence {
    pub(crate) fn promote(target: &mut ParameterEvidence, evidence: ParameterEvidence) {
        if evidence > *target {
            *target = evidence;
        }
    }
}

#[derive(Default)]
pub struct ParamRepairState {
    pub h264_ps: Option<H264ParameterSets>,
    pub h265_ps: Option<H265ParameterSets>,
    pub aac_asc: Option<[u8; 2]>,
    pub ready: bool,
    pub(crate) evidence: TrackParameterEvidence,
}

impl ParamRepairState {
    unsafe fn from_stream(stream: *mut AVStream) -> Self {
        let mut state = Self::default();
        if stream.is_null() || unsafe { (*stream).codecpar.is_null() } {
            return state;
        }
        let codecpar = unsafe { (*stream).codecpar };
        if unsafe { (*codecpar).codec_id } != AVCodecID_AV_CODEC_ID_NONE {
            state.evidence.codec = ParameterEvidence::Demux;
        }
        if unsafe { (*codecpar).sample_rate } > 0 {
            state.evidence.sample_rate = ParameterEvidence::Demux;
        }
        if unsafe { (*codecpar).ch_layout.nb_channels } > 0 || unsafe { (*codecpar).channels } > 0 {
            state.evidence.channels = ParameterEvidence::Demux;
        }
        if unsafe { (*codecpar).extradata_size } > 0 && !unsafe { (*codecpar).extradata.is_null() }
        {
            state.evidence.extradata = ParameterEvidence::Demux;
        }
        if unsafe { (*stream).time_base.num } > 0 && unsafe { (*stream).time_base.den } > 0 {
            state.evidence.time_base = ParameterEvidence::Demux;
        }
        state
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

/// Helper: create an AVFormatContext and set custom IO flag
unsafe fn alloc_fmt_ctx_with_custom_io(
    read_control: &Arc<rtp::RtpReadControl>,
) -> GlobalResult<*mut AVFormatContext> {
    unsafe {
        let fmt_ctx = avformat_alloc_context();
        if fmt_ctx.is_null() {
            return Err(GlobalError::new_sys_error(
                "Failed to alloc format context",
                |msg| error!("{msg}"),
            ));
        }
        // mark we will use custom IO
        (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;
        (*fmt_ctx).interrupt_callback.callback = Some(rw::interrupt_rtp_read);
        (*fmt_ctx).interrupt_callback.opaque =
            Arc::as_ptr(read_control).cast_mut().cast::<c_void>();
        Ok(fmt_ctx)
    }
}

/// Helper: allocate AVIOContext with boxed opaque tuple; returns (pb, boxed_ptr, buf_ptr)
unsafe fn alloc_avio_for_rtp(
    rtp_buffer: rtp::RtpPacketBuffer,
) -> GlobalResult<(*mut AVIOContext, *mut c_void, *mut u8)> {
    unsafe {
        // allocate IO buffer
        let io_buf = rsmpeg::ffi::av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
        if io_buf.is_null() {
            return Err(GlobalError::new_sys_error(
                "Failed to allocate IO buffer",
                |msg| error!("{msg}"),
            ));
        }

        let boxed = Box::new(rtp_buffer);
        let opaque = Box::into_raw(boxed) as *mut c_void;

        // create avio ctx
        let pb = avio_alloc_context(
            io_buf,
            DEFAULT_IO_BUF_SIZE as c_int,
            0,
            opaque,
            Some(rw::read_rtp_payload), // your read callback
            None,
            None,
        );
        if pb.is_null() {
            // cleanup: free io_buf and boxed opaque
            // restore Box to drop it
            let tup = opaque as OpaquePtr;
            drop(Box::from_raw(tup));
            av_free(io_buf as *mut c_void);
            return Err(GlobalError::new_sys_error(
                "Failed to allocate AVIO context",
                |msg| error!("{msg}"),
            ));
        }

        // ensure pb isn't marked seekable
        (*pb).seekable = 0;
        Ok((pb, opaque, io_buf))
    }
}

/// Probe once. The custom AVIO and interrupt callback own waiting and cancellation.
unsafe fn find_stream_info(
    fmt_ctx: *mut AVFormatContext,
    dict_opts: *mut AVDictionary,
) -> Result<(), GlobalError> {
    unsafe {
        let ret = avformat_find_stream_info(fmt_ctx, &mut (dict_opts as *mut _));
        if ret < 0 {
            let detail = show_ffmpeg_error_msg(ret);
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                &format!("failed to find stream info: {detail}"),
                |msg| error!("{msg}"),
            ));
        }
        Ok(())
    }
}

/// Helper: cleanup when start_demuxer fails before AvioResource is returned.
/// This will free io_ctx (if non-null), io_buf (if non-null) and boxed opaque (if non-null).
unsafe fn cleanup_early(io_ctx: *mut AVIOContext, opaque_ptr: *mut c_void, io_buf: *mut u8) {
    unsafe {
        // If avio ctx exists, clear its opaque and free it
        if !io_ctx.is_null() {
            // detach opaque to avoid double-free inside avio_context_free
            (*io_ctx).opaque = ptr::null_mut();
            let mut local_io = io_ctx;
            avio_context_free(&mut local_io); // sets to NULL
        } else if !io_buf.is_null() {
            av_free(io_buf as *mut c_void);
        }
        // drop boxed opaque if allocated
        if !opaque_ptr.is_null() {
            let tup = opaque_ptr as OpaquePtr;
            // Safety: only call when we are sure AvioResource was not created to own it.
            drop(Box::from_raw(tup));
        }
    }
}

/// Map helper functions (you provided earlier, included here for completeness)
unsafe fn map_video_codec_id(s: &str) -> AVCodecID {
    match s.to_lowercase().as_str() {
        "h264" | "h.264" | "avc" => AVCodecID_AV_CODEC_ID_H264,
        "h265" | "h.265" | "hevc" => AVCodecID_AV_CODEC_ID_HEVC,
        "mpeg4" => AVCodecID_AV_CODEC_ID_MPEG4,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        "3gp" => AVCodecID_AV_CODEC_ID_H263, // 视来源定义
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

fn supported_audio_codec(codec_id: AVCodecID) -> bool {
    matches!(
        codec_id,
        AVCodecID_AV_CODEC_ID_AAC
            | AVCodecID_AV_CODEC_ID_PCM_ALAW
            | AVCodecID_AV_CODEC_ID_PCM_MULAW
            | AVCodecID_AV_CODEC_ID_G723_1
            | AVCodecID_AV_CODEC_ID_G729
    )
}

fn audio_codec_hint<'a>(media_ext: &'a MediaExt, fmt_name: &str) -> Option<(&'a str, AVCodecID)> {
    let declaration = &media_ext.declaration.audio;
    let codec = if fmt_name == "mpeg" {
        (declaration.is_active() && declaration.embedded_in_ps)
            .then_some(declaration.codec.as_deref())
            .flatten()
    } else if declaration.is_active() {
        declaration
            .codec
            .as_deref()
            .or(media_ext.audio_params.codec_id.as_deref())
    } else {
        media_ext.audio_params.codec_id.as_deref()
    }?;
    let codec_id = unsafe { map_audio_codec_id(codec) };
    supported_audio_codec(codec_id).then_some((codec, codec_id))
}

pub unsafe fn map_audio_codec_id(s: &str) -> AVCodecID {
    match s.to_lowercase().as_str() {
        // G.711 A-law
        "g711" | "g711a" | "g.711a" | "g.711 a-law" | "a-law" | "alaw" | "pcma" | "pcm_alaw" => {
            AVCodecID_AV_CODEC_ID_PCM_ALAW
        }
        // G.711 μ-law
        "g711 u-law" | "g.711 u-law" | "u-law" | "ulaw" => AVCodecID_AV_CODEC_ID_PCM_MULAW,
        "g711u" | "g.711u" | "g.711 μ-law" | "mu-law" | "mulaw" | "pcmu" | "pcm_mulaw" => {
            AVCodecID_AV_CODEC_ID_PCM_MULAW
        }
        // G.722
        "g722" | "g.722" => AVCodecID_AV_CODEC_ID_ADPCM_G722,
        // G.722.1 (Siren)
        "g7221" | "g.722.1" | "siren" => AVCodecID_AV_CODEC_ID_SIREN,
        // G.723.1
        "g723" | "g7231" | "g.723" | "g.723.1" | "g723_1" => AVCodecID_AV_CODEC_ID_G723_1,
        // G.729
        "g729" | "g.729" => AVCodecID_AV_CODEC_ID_G729,
        // AAC
        "aac" | "mpeg2-aac" | "mpeg4-aac" => AVCodecID_AV_CODEC_ID_AAC,
        // "svac" => AVCodecID_AV_CODEC_ID_SVAC,//avcodec_find_decoder_by_name("svac")
        _ => AVCodecID_AV_CODEC_ID_NONE,
    }
}

/// pick input format (reuse your logic)
fn pick_input_format(media_ext: &MediaExt) -> &'static str {
    let type_name = media_ext.type_name.to_ascii_uppercase();
    match type_name.as_str() {
        "PS" | "MP2P" | "MP2PS" => "mpeg", // mpeg-ps
        "H264" | "H.264" | "AVC" => "h264",
        "H265" | "H.265" | "HEVC" => "hevc",
        "AAC" => "aac",
        "G711U" | "PCMU" | "MULAW" => "mulaw",
        "G711" | "G711A" | "PCMA" | "ALAW" => "alaw",
        _ => {
            if media_ext
                .video_params
                .codec_id
                .as_deref()
                .map(|s| {
                    s.eq_ignore_ascii_case("h264")
                        || s.eq_ignore_ascii_case("h.264")
                        || s.eq_ignore_ascii_case("avc")
                })
                .unwrap_or(false)
            {
                "h264"
            } else if media_ext
                .video_params
                .codec_id
                .as_deref()
                .map(|s| {
                    s.eq_ignore_ascii_case("h265")
                        || s.eq_ignore_ascii_case("h.265")
                        || s.eq_ignore_ascii_case("hevc")
                })
                .unwrap_or(false)
            {
                "hevc"
            } else if media_ext
                .audio_params
                .codec_id
                .as_deref()
                .map(is_mulaw_codec)
                .unwrap_or(false)
            {
                "mulaw"
            } else if media_ext
                .audio_params
                .codec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("aac"))
                .unwrap_or(false)
            {
                "aac"
            } else if media_ext.audio_params.codec_id.is_some() {
                "alaw"
            } else {
                "mpeg"
            }
        }
    }
}

fn is_mulaw_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "g711u"
            | "g.711u"
            | "g711 u-law"
            | "g.711 u-law"
            | "u-law"
            | "ulaw"
            | "mu-law"
            | "mulaw"
            | "pcmu"
            | "pcm_mulaw"
    )
}

fn input_sample_rate(media_ext: &MediaExt) -> i32 {
    media_ext
        .audio_params
        .sample_rate
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|rate| {
            if rate > 0.0 && rate < 1000.0 {
                (rate * 1000.0).round() as i32
            } else {
                rate.round() as i32
            }
        })
        .filter(|rate| *rate > 0)
        .or_else(|| {
            media_ext
                .declaration
                .audio
                .clock_rate
                .filter(|rate| *rate > 0)
        })
        .or_else(|| {
            if media_ext.audio_params.clock_rate > 0 {
                Some(media_ext.audio_params.clock_rate)
            } else {
                None
            }
        })
        .unwrap_or(8000)
}

unsafe fn apply_embedded_audio_declaration(
    fmt_ctx: *mut AVFormatContext,
    media_ext: &MediaExt,
    params: &mut [ParamRepairState],
) {
    let declaration = &media_ext.declaration.audio;
    let Some((declared, declared_id)) = audio_codec_hint(media_ext, "mpeg") else {
        return;
    };
    for index in 0..unsafe { (*fmt_ctx).nb_streams as usize } {
        let stream = unsafe { *(*fmt_ctx).streams.add(index) };
        if stream.is_null() || unsafe { (*stream).codecpar.is_null() } {
            continue;
        }
        let codecpar = unsafe { (*stream).codecpar };
        if unsafe { (*codecpar).codec_type } != AVMediaType_AVMEDIA_TYPE_AUDIO {
            continue;
        }
        let observed_id = unsafe { (*codecpar).codec_id };
        if observed_id != AVCodecID_AV_CODEC_ID_NONE && observed_id != declared_id {
            warn!(
                "audio parameter mismatch: field=codec, declared={declared}, observed_id={observed_id}, outcome=declared_wins, reason=embedded_ps_f_field"
            );
        }
        if observed_id != declared_id {
            unsafe {
                if !(*codecpar).extradata.is_null() {
                    av_free((*codecpar).extradata.cast());
                    (*codecpar).extradata = ptr::null_mut();
                    (*codecpar).extradata_size = 0;
                }
                (*codecpar).codec_id = declared_id;
                (*codecpar).codec_tag = 0;
                (*codecpar).format = -1;
                (*codecpar).frame_size = 0;
                (*codecpar).block_align = 0;
                (*codecpar).bits_per_coded_sample = 0;
                (*codecpar).sample_rate = 0;
                (*codecpar).channels = 0;
                (*codecpar).channel_layout = 0;
                av_channel_layout_uninit(&mut (*codecpar).ch_layout);
            }
        }
        if let Some(sample_rate) = declaration.clock_rate.filter(|rate| *rate > 0) {
            unsafe { (*codecpar).sample_rate = sample_rate };
        }
        let channels = declaration
            .channels
            .filter(|channels| *channels > 0)
            .or_else(|| {
                matches!(
                    declared_id,
                    AVCodecID_AV_CODEC_ID_PCM_ALAW
                        | AVCodecID_AV_CODEC_ID_PCM_MULAW
                        | AVCodecID_AV_CODEC_ID_G723_1
                        | AVCodecID_AV_CODEC_ID_G729
                )
                .then_some(1)
            });
        if let Some(channels) = channels {
            unsafe {
                (*codecpar).channels = channels;
                (*codecpar).channel_layout =
                    rsmpeg::ffi::av_get_default_channel_layout(channels) as u64;
                av_channel_layout_uninit(&mut (*codecpar).ch_layout);
                av_channel_layout_default(&mut (*codecpar).ch_layout, channels);
            }
        }
        if let Some(bit_rate) = media_ext
            .audio_params
            .bitrate
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| (value * 1_000.0).round() as i64)
            .filter(|value| *value > 0)
        {
            unsafe { (*codecpar).bit_rate = bit_rate };
        }
        if matches!(
            declared_id,
            AVCodecID_AV_CODEC_ID_PCM_ALAW | AVCodecID_AV_CODEC_ID_PCM_MULAW
        ) {
            unsafe {
                (*codecpar).bits_per_coded_sample = 8;
                (*codecpar).block_align = 1;
            }
        }
        if let Some(param) = params.get_mut(index) {
            param.ready = false;
            param.evidence.codec = ParameterEvidence::Sdp;
            if unsafe { (*codecpar).sample_rate } > 0 {
                param.evidence.sample_rate = ParameterEvidence::Sdp;
            }
            if unsafe { (*codecpar).channels } > 0 {
                param.evidence.channels = ParameterEvidence::Sdp;
            }
        }
        info!(
            "audio codec resolved: action=input_probe, outcome=declared_confirmed, source=f_field, codec={declared}, codec_id={declared_id}, sample_rate={}, channels={}",
            unsafe { (*codecpar).sample_rate },
            unsafe { (*codecpar).channels }
        );
        return;
    }
}

/// The refactored start_demuxer broken into steps; returns DemuxerContext on success.
impl DemuxerContext {
    pub fn start_demuxer(
        _ssrc: u32,
        media_ext: &MediaExt,
        rtp_buffer: rtp::RtpPacketBuffer,
        read_control: Arc<rtp::RtpReadControl>,
    ) -> GlobalResult<Self> {
        unsafe {
            // 0) pre-checks
            // allocate fmt_ctx
            let in_fmt_ctx = alloc_fmt_ctx_with_custom_io(&read_control)?;

            // 1) pick input format
            let fmt_name = pick_input_format(media_ext);
            debug!("Using input format: {}", fmt_name);
            let ifmt_name = CString::new(fmt_name).unwrap();
            let input_fmt = av_find_input_format(ifmt_name.as_ptr());
            if input_fmt.is_null() {
                avformat_free_context(in_fmt_ctx);
                return Err(GlobalError::new_sys_error(
                    &format!("demuxer not found: {}", fmt_name),
                    |msg| error!("{msg}"),
                ));
            }

            // 2) alloc avio + boxed opaque
            let (in_pb, in_opaque, in_io_buf) = match alloc_avio_for_rtp(rtp_buffer) {
                Ok(t) => t,
                Err(e) => {
                    avformat_free_context(in_fmt_ctx);
                    return Err(e);
                }
            };

            // attach pb to fmt_ctx
            (*in_fmt_ctx).pb = in_pb;

            // 3) set codec hints if provided in media_ext
            if let Some(v_id) = &media_ext.video_params.codec_id {
                let id = map_video_codec_id(v_id);
                if id != AVCodecID_AV_CODEC_ID_NONE {
                    (*in_fmt_ctx).video_codec_id = id;
                    let codec = avcodec_find_decoder(id);
                    if codec.is_null() {
                        // cleanup: free resources we just allocated
                        cleanup_early(in_pb, in_opaque, in_io_buf);
                        avformat_free_context(in_fmt_ctx);
                        return Err(GlobalError::new_sys_error(
                            &format!("Video codec not found: {}", v_id),
                            |msg| error!("{msg}"),
                        ));
                    }
                    (*in_fmt_ctx).video_codec = codec;
                }
            }
            if let Some((audio_codec, id)) = audio_codec_hint(media_ext, fmt_name) {
                (*in_fmt_ctx).audio_codec_id = id;
                let codec = avcodec_find_decoder(id);
                if codec.is_null() {
                    warn!(
                        "audio decoder unavailable: action=input_probe, outcome=video_continues, codec={audio_codec}"
                    );
                } else {
                    (*in_fmt_ctx).audio_codec = codec;
                }
            }

            // 4) build dictionary options
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();
            macro_rules! set_dict {
                ($k:expr, $v:expr) => {{
                    let key = CString::new($k).unwrap();
                    let val = CString::new($v).unwrap();
                    rsmpeg::ffi::av_dict_set(&mut dict_opts, key.as_ptr(), val.as_ptr(), 0);
                }};
            }
            //分离探测与修复
            //不设置 fflags=nobuffer+igndts+genpts，启用默认缓存机制,用来后继修复及归一化时间
            //avformat_find_stream_info 仅完成最小化探测（识别流数量和类型）
            //后续通过自己的 read pkt 逻辑来补充和完善流信息
            set_dict!("fflags", "discardcorrupt+genpts");
            set_dict!("analyzeduration", "1000000");
            set_dict!("probesize", "65536");
            set_dict!("max_probe_packets", "64");
            if matches!(fmt_name, "alaw" | "mulaw") {
                let sample_rate = input_sample_rate(media_ext).to_string();
                let channels = media_ext.audio_params.channel_count.max(1).to_string();
                set_dict!("sample_rate", sample_rate.as_str());
                set_dict!("channels", channels.as_str());
                set_dict!(
                    "ch_layout",
                    if media_ext.audio_params.channel_count == 2 {
                        "stereo"
                    } else {
                        "mono"
                    }
                );
            }

            // 5) open input
            let open_ret = avformat_open_input(
                &mut (in_fmt_ctx as *mut _),
                ptr::null(),
                input_fmt,
                &mut dict_opts,
            );
            if open_ret < 0 {
                // cleanup: free dict, avio, boxed opaque, io_buf, fmt_ctx
                rsmpeg::ffi::av_dict_free(&mut dict_opts);
                // avio_context_free and drop opaque handled by cleanup_early
                cleanup_early(in_pb, in_opaque, in_io_buf);
                avformat_free_context(in_fmt_ctx);
                let ffmpeg_error = show_ffmpeg_error_msg(open_ret);
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    &ffmpeg_error,
                    |msg| error!("{msg}"),
                ));
            }
            let avio = AvioResource {
                fmt_ctx: in_fmt_ctx,
                io_buf: in_io_buf,
                avio_ctx: in_pb,
            };
            // 6) find stream info once; AVIO handles waiting and interruption.
            if let Err(e) = find_stream_info(avio.fmt_ctx, dict_opts) {
                rsmpeg::ffi::av_dict_free(&mut dict_opts);
                drop(avio);
                return Err(e);
            }
            // 8) update probe cache & collect codecpar_list & stream mapping & reset_timestamp_state
            let nb_streams = (*in_fmt_ctx).nb_streams as usize;
            let mut params: Vec<ParamRepairState> = (0..nb_streams)
                .map(|index| ParamRepairState::from_stream(*(*in_fmt_ctx).streams.add(index)))
                .collect();
            if fmt_name == "mpeg" {
                apply_embedded_audio_declaration(in_fmt_ctx, media_ext, &mut params);
            }
            rsmpeg::ffi::av_dict_free(&mut dict_opts);
            Ok(DemuxerContext {
                avio,
                params,
                read_control,
                output_plan: OutputTrackPlan::default(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputTrack, OutputTrackPlan, OutputTrackSource, ParamRepairState, ParameterEvidence,
        apply_embedded_audio_declaration, audio_codec_hint, extend_param_repair_states,
        map_audio_codec_id, ready_for_initial_output,
    };
    use gmv_domain::info::media_info_ext::{MediaDeclarationState, MediaExt};
    use rsmpeg::ffi::{
        AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_ADPCM_G722, AVCodecID_AV_CODEC_ID_G723_1,
        AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
        AVCodecID_AV_CODEC_ID_MP2, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW,
        AVCodecID_AV_CODEC_ID_SIREN, AVMediaType_AVMEDIA_TYPE_AUDIO,
        AVMediaType_AVMEDIA_TYPE_VIDEO, AVOption, av_demuxer_iterate, av_opt_next,
        avcodec_find_decoder, avcodec_find_encoder, avformat_alloc_context, avformat_free_context,
        avformat_new_stream,
    };
    use std::ffi::CStr;
    use std::ptr;

    #[test]
    fn extends_param_repair_state_when_demuxer_discovers_stream() {
        let mut params = vec![ParamRepairState {
            ready: true,
            ..Default::default()
        }];

        assert_eq!(extend_param_repair_states(&mut params, 2), 1..2);
        assert_eq!(params.len(), 2);
        assert!(params[0].ready);
        assert!(!params[1].ready);
        assert_eq!(extend_param_repair_states(&mut params, 1), 2..2);
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn silent_audio_track_is_added_once_for_late_generation() {
        let mut plan = OutputTrackPlan::default();

        plan.add_silent_audio();
        plan.add_silent_audio();

        assert!(plan.has_audio());
        assert!(plan.has_silent_audio());
        assert_eq!(plan.tracks.len(), 1);
    }

    #[test]
    fn transcoded_audio_keeps_input_packet_index_but_uses_fixed_output_profile() {
        let mut plan = OutputTrackPlan {
            tracks: vec![OutputTrack {
                packet_index: 1,
                source: OutputTrackSource::Input(1),
                media_type: AVMediaType_AVMEDIA_TYPE_AUDIO,
            }],
        };

        plan.mark_transcoded_audio(1);

        assert_eq!(plan.tracks[0].source, OutputTrackSource::TranscodedAac(1));
        assert!(plan.contains_packet_index(1));
        assert!(plan.has_fixed_aac());
        assert!(!plan.has_silent_audio());
    }

    #[test]
    fn discovered_video_can_finish_parameter_repair_after_audio_window_freezes() {
        let snapshot = [false, false];

        assert!(ready_for_initial_output(
            AVMediaType_AVMEDIA_TYPE_VIDEO,
            0,
            true,
            &snapshot,
        ));
        assert!(!ready_for_initial_output(
            AVMediaType_AVMEDIA_TYPE_AUDIO,
            1,
            true,
            &snapshot,
        ));
        assert!(!ready_for_initial_output(
            AVMediaType_AVMEDIA_TYPE_VIDEO,
            2,
            true,
            &snapshot,
        ));
    }

    #[test]
    fn gb_audio_codec_names_map_without_conflating_g722_and_g7221() {
        unsafe {
            assert_eq!(map_audio_codec_id("g7221"), AVCodecID_AV_CODEC_ID_SIREN);
            assert_eq!(map_audio_codec_id("siren"), AVCodecID_AV_CODEC_ID_SIREN);
            assert_eq!(map_audio_codec_id("g7231"), AVCodecID_AV_CODEC_ID_G723_1);
            assert_eq!(map_audio_codec_id("g729"), AVCodecID_AV_CODEC_ID_G729);
        }
    }

    fn embedded_g711_media_ext() -> MediaExt {
        let mut media_ext = MediaExt::default();
        media_ext.type_name = "PS".to_string();
        media_ext.audio_params.codec_id = Some("g711".to_string());
        media_ext.audio_params.bitrate = Some("64".to_string());
        media_ext.audio_params.sample_rate = Some("8".to_string());
        media_ext.declaration.audio.state = MediaDeclarationState::Active;
        media_ext.declaration.audio.codec = Some("g711".to_string());
        media_ext.declaration.audio.clock_rate = Some(8_000);
        media_ext.declaration.audio.embedded_in_ps = true;
        media_ext
    }

    #[test]
    fn embedded_ps_f_field_supplies_supported_audio_hint() {
        let mut media_ext = embedded_g711_media_ext();
        for (codec, codec_id) in [
            ("g711", AVCodecID_AV_CODEC_ID_PCM_ALAW),
            ("g711u", AVCodecID_AV_CODEC_ID_PCM_MULAW),
            ("g7231", AVCodecID_AV_CODEC_ID_G723_1),
            ("g729", AVCodecID_AV_CODEC_ID_G729),
            ("aac", AVCodecID_AV_CODEC_ID_AAC),
        ] {
            media_ext.declaration.audio.codec = Some(codec.to_string());
            assert_eq!(
                audio_codec_hint(&media_ext, "mpeg").map(|(_, codec_id)| codec_id),
                Some(codec_id),
                "supported f= codec was rejected: {codec}"
            );
        }

        media_ext.declaration.audio.codec = Some("g7221".to_string());
        assert!(audio_codec_hint(&media_ext, "mpeg").is_none());

        let mut not_embedded = embedded_g711_media_ext();
        not_embedded.declaration.audio.embedded_in_ps = false;
        assert!(audio_codec_hint(&not_embedded, "mpeg").is_none());
    }

    #[test]
    fn embedded_ps_f_field_overrides_ambiguous_demux_audio() {
        unsafe {
            let format = avformat_alloc_context();
            let stream = avformat_new_stream(format, ptr::null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_ADPCM_G722;
            (*codecpar).sample_rate = 44_100;
            (*codecpar).channels = 2;
            let mut params = vec![ParamRepairState::from_stream(stream)];

            apply_embedded_audio_declaration(format, &embedded_g711_media_ext(), &mut params);

            assert_eq!((*codecpar).codec_id, AVCodecID_AV_CODEC_ID_PCM_ALAW);
            assert_eq!((*codecpar).sample_rate, 8_000);
            assert_eq!((*codecpar).channels, 1);
            assert_eq!((*codecpar).bit_rate, 64_000);
            assert_eq!((*codecpar).bits_per_coded_sample, 8);
            assert_eq!((*codecpar).block_align, 1);
            assert_eq!(params[0].evidence.codec, ParameterEvidence::Sdp);
            assert_eq!(params[0].evidence.sample_rate, ParameterEvidence::Sdp);
            assert_eq!(params[0].evidence.channels, ParameterEvidence::Sdp);
            avformat_free_context(format);
        }
    }

    #[test]
    fn runtime_ffmpeg_contains_promised_audio_transcode_capabilities() {
        unsafe {
            for codec_id in [
                AVCodecID_AV_CODEC_ID_H264,
                AVCodecID_AV_CODEC_ID_HEVC,
                AVCodecID_AV_CODEC_ID_AAC,
                AVCodecID_AV_CODEC_ID_PCM_ALAW,
                AVCodecID_AV_CODEC_ID_PCM_MULAW,
                AVCodecID_AV_CODEC_ID_G723_1,
                AVCodecID_AV_CODEC_ID_G729,
            ] {
                assert!(
                    !avcodec_find_decoder(codec_id).is_null(),
                    "required decoder missing for codec id {codec_id}"
                );
            }
            assert!(
                !avcodec_find_encoder(AVCodecID_AV_CODEC_ID_AAC).is_null(),
                "required AAC encoder missing"
            );
            assert!(
                avcodec_find_decoder(AVCodecID_AV_CODEC_ID_MP2).is_null(),
                "unsupported MP2 decoder must be excluded from the minimal FFmpeg build"
            );
        }
    }

    #[test]

    fn for_supported_demuxer() {
        unsafe {
            let mut opaque = ptr::null_mut();
            while let Some(fmt) = av_demuxer_iterate(&mut opaque).as_ref() {
                let fmt_name = CStr::from_ptr((*fmt).name).to_string_lossy();
                let fmt_long_name = CStr::from_ptr((*fmt).long_name).to_string_lossy();
                println!("Supported demuxer: {}, {}", fmt_name, fmt_long_name);
            }
        }
    }

    #[test]
    fn for_enum_protocols() {
        unsafe {
            let mut opaque = ptr::null_mut();
            println!("Input protocols:");
            while let Some(protocol) = rsmpeg::ffi::avio_enum_protocols(&mut opaque, 0).as_ref() {
                let protocol_name = CStr::from_ptr(protocol).to_string_lossy();
                println!("  - {}", protocol_name);
            }
            println!("\nOutput protocols:");
            let mut opaque = ptr::null_mut();
            while let Some(protocol) = rsmpeg::ffi::avio_enum_protocols(&mut opaque, 1).as_ref() {
                let protocol_name = CStr::from_ptr(protocol).to_string_lossy();
                println!("  - {}", protocol_name);
            }
        }
    }
    #[test]
    fn dump_avoptions_for_format_context() {
        unsafe {
            let fmt_ctx = rsmpeg::ffi::avformat_alloc_context();
            let mut opt: *const rsmpeg::ffi::AVOption = std::ptr::null();
            let obj = fmt_ctx as *mut std::ffi::c_void;

            while {
                opt = rsmpeg::ffi::av_opt_next(obj, opt);
                !opt.is_null()
            } {
                let o = &*opt;
                let name = std::ffi::CStr::from_ptr(o.name).to_string_lossy();
                let help = if !o.help.is_null() {
                    std::ffi::CStr::from_ptr(o.help)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "".to_string()
                };
                println!(
                    "option: {} (help: {}, type: {}, min: {}, max: {})",
                    name, help, o.type_, o.min, o.max
                );
            }

            rsmpeg::ffi::avformat_free_context(fmt_ctx);
        }
    }

    /// 打印所有可用 demuxer 及其支持的参数
    #[test]
    fn dump_all_demuxer_options() {
        unsafe {
            let mut opaque: *mut std::ffi::c_void = ptr::null_mut();

            loop {
                let ifmt = av_demuxer_iterate(&mut opaque);
                if ifmt.is_null() {
                    break;
                }
                let name = if !(*ifmt).name.is_null() {
                    CStr::from_ptr((*ifmt).name).to_string_lossy().into_owned()
                } else {
                    "<unknown>".to_string()
                };

                println!("Demuxer: {}", name);

                let av_class = (*ifmt).priv_class;
                if !av_class.is_null() {
                    let mut opt: *const AVOption = ptr::null();
                    loop {
                        opt = av_opt_next(ptr::null(), opt);
                        if opt.is_null() {
                            break;
                        }
                        let opt_name = if !(*opt).name.is_null() {
                            CStr::from_ptr((*opt).name).to_string_lossy()
                        } else {
                            std::borrow::Cow::Borrowed("<noname>")
                        };
                        println!("    option: {}", opt_name);
                    }
                }
            }
        }
    }
}
