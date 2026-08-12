use crate::media::context::format::demuxer::{DemuxerContext, SYNTHETIC_AUDIO_PACKET_INDEX};
use crate::media::context::format::{
    OUTPUT_AAC_BIT_RATE, OUTPUT_AAC_CHANNELS, OUTPUT_AAC_SAMPLE_RATE,
};
use crate::state::layer::codec_layer::CodecLayer;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use gmv_domain::info::media_info::{OutputAudioCodec, TranscodeConfig};
use rsmpeg::avcodec::{AVCodec, AVCodecContext, AVPacket as OwnedPacket};
use rsmpeg::avutil::{AVAudioFifo, AVFrame, AVRational, av_get_default_channel_layout};
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi::{
    AV_CODEC_FLAG_GLOBAL_HEADER, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_G723_1,
    AVCodecID_AV_CODEC_ID_G729, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
    AVCodecID_AV_CODEC_ID_MP2, AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_PCM_ALAW,
    AVCodecID_AV_CODEC_ID_PCM_MULAW, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, FF_PROFILE_AAC_LOW, av_packet_ref,
    av_samples_set_silence, avcodec_parameters_alloc, avcodec_parameters_copy,
    avcodec_parameters_free, avcodec_parameters_to_context,
};
use rsmpeg::swresample::SwrContext;

const AAC_SAMPLE_RATE: i32 = OUTPUT_AAC_SAMPLE_RATE;
const AAC_CHANNELS: i32 = OUTPUT_AAC_CHANNELS;
const AAC_BIT_RATE: i64 = OUTPUT_AAC_BIT_RATE;
pub(crate) const AUDIO_STALL_GRACE_US: i64 = 500_000;
const AUDIO_RECOVERY_FRAMES: usize = 8;

pub struct CodecContext {
    target_audio: Option<OutputAudioCodec>,
    audio: Option<AacTranscoder>,
    silent: Option<SilentAacSource>,
    last_real_audio_us: Option<i64>,
    recovery_frames: usize,
    rejected_audio_stream: Option<i32>,
    source_audio_parameters: Option<CodecParametersSnapshot>,
}

impl CodecContext {
    pub fn init(
        _legacy_codec: Option<CodecLayer>,
        transcode: Option<TranscodeConfig>,
    ) -> Option<Self> {
        let target_audio = transcode.and_then(|config| config.audio_codec);
        target_audio.map(|target_audio| Self {
            target_audio: Some(target_audio),
            audio: None,
            silent: None,
            last_real_audio_us: None,
            recovery_frames: 0,
            rejected_audio_stream: None,
            source_audio_parameters: None,
        })
    }

    pub fn fixed_aac() -> Self {
        Self {
            target_audio: Some(OutputAudioCodec::Aac),
            audio: None,
            silent: None,
            last_real_audio_us: None,
            recovery_frames: 0,
            rejected_audio_stream: None,
            source_audio_parameters: None,
        }
    }

    pub unsafe fn prepare(
        &mut self,
        demuxer: &mut DemuxerContext,
        audio_expected: bool,
        ready_at_discovery: &[bool],
    ) -> GlobalResult<()> {
        unsafe {
            let fmt_ctx = demuxer.avio.fmt_ctx;
            let mut audio_stream = None;
            for index in 0..(*fmt_ctx).nb_streams as usize {
                let stream = *(*fmt_ctx).streams.add(index);
                let codecpar = (*stream).codecpar;
                match (*codecpar).codec_type {
                    AVMediaType_AVMEDIA_TYPE_VIDEO => {
                        if !matches!(
                            (*codecpar).codec_id,
                            AVCodecID_AV_CODEC_ID_H264 | AVCodecID_AV_CODEC_ID_HEVC
                        ) {
                            return Err(unsupported(
                                "UNSUPPORTED_VIDEO_CODEC",
                                format!("unsupported video codec id {}", (*codecpar).codec_id),
                            ));
                        }
                    }
                    AVMediaType_AVMEDIA_TYPE_AUDIO => {
                        if (*codecpar).codec_id != AVCodecID_AV_CODEC_ID_NONE
                            && !supported_audio_source((*codecpar).codec_id)
                        {
                            self.rejected_audio_stream.get_or_insert(index as i32);
                            continue;
                        }
                        if audio_stream.is_none()
                            && ready_at_discovery.get(index).copied().unwrap_or(false)
                        {
                            audio_stream = Some((index as i32, stream));
                        }
                    }
                    _ => {}
                }
            }

            if self.target_audio != Some(OutputAudioCodec::Aac) {
                return Ok(());
            }
            let Some((stream_index, stream)) = audio_stream else {
                if audio_expected {
                    self.silent = silent_aac_or_warn(SYNTHETIC_AUDIO_PACKET_INDEX);
                }
                return Ok(());
            };
            let codecpar = (*stream).codecpar;
            match (*codecpar).codec_id {
                AVCodecID_AV_CODEC_ID_AAC => Ok(()),
                AVCodecID_AV_CODEC_ID_PCM_ALAW
                | AVCodecID_AV_CODEC_ID_PCM_MULAW
                | AVCodecID_AV_CODEC_ID_G723_1
                | AVCodecID_AV_CODEC_ID_G729
                | AVCodecID_AV_CODEC_ID_MP2 => {
                    let transcode = CodecParametersSnapshot::copy(codecpar).and_then(|snapshot| {
                        AacTranscoder::new(stream_index, stream_index, snapshot.as_ptr(), 0)
                            .map(|audio| (audio, snapshot))
                    });
                    match transcode {
                        Ok((audio, snapshot)) => {
                            self.audio = Some(audio);
                            self.source_audio_parameters = Some(snapshot);
                            self.silent = silent_aac_or_warn(stream_index);
                        }
                        Err(error) => {
                            warn!(
                                "audio transcode initialization failed: action=audio_transcode, outcome=silent_placeholder, stream_index={stream_index}, reason={error}"
                            );
                            self.rejected_audio_stream = Some(stream_index);
                            self.silent = silent_aac_or_warn(SYNTHETIC_AUDIO_PACKET_INDEX);
                        }
                    }
                    Ok(())
                }
                _ => {
                    self.rejected_audio_stream = Some(stream_index);
                    if audio_expected {
                        self.silent = silent_aac_or_warn(SYNTHETIC_AUDIO_PACKET_INDEX);
                    }
                    Ok(())
                }
            }
        }
    }

    pub fn rejected_audio_stream(&self) -> Option<i32> {
        self.rejected_audio_stream
    }

    pub fn has_output_audio(&self) -> bool {
        self.audio.is_some() || self.silent.is_some()
    }

    pub fn enable_late_placeholder(&mut self) -> GlobalResult<()> {
        if self.silent.is_none() {
            self.silent = Some(SilentAacSource::new(SYNTHETIC_AUDIO_PACKET_INDEX)?);
        }
        Ok(())
    }

    pub fn handles(&self, packet: &AVPacket) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.stream_index == packet.stream_index)
    }

    pub fn has_silent_audio(&self) -> bool {
        self.silent.is_some()
    }

    pub fn has_real_audio(&self) -> bool {
        self.audio.is_some()
    }

    pub fn transcoded_stream_index(&self) -> Option<usize> {
        self.audio.as_ref().map(|audio| audio.stream_index as usize)
    }

    pub unsafe fn observe_ready_audio(
        &mut self,
        demuxer: &mut DemuxerContext,
        source_index: i32,
        output_index: i32,
    ) -> GlobalResult<bool> {
        if self.silent.is_none() || self.audio.is_some() {
            return Ok(false);
        }
        self.recovery_frames = self.recovery_frames.saturating_add(1);
        if self.recovery_frames < AUDIO_RECOVERY_FRAMES {
            return Ok(false);
        }
        let stream = unsafe { *(*demuxer.avio.fmt_ctx).streams.add(source_index as usize) };
        if !supported_audio_source(unsafe { (*(*stream).codecpar).codec_id }) {
            return Ok(false);
        }
        let next_pts = self
            .silent
            .as_ref()
            .map_or(0, |silent| silent.next_packet_pts);
        let new_snapshot = if self.source_audio_parameters.is_none() {
            Some(CodecParametersSnapshot::copy(unsafe {
                (*stream).codecpar
            })?)
        } else {
            None
        };
        let source_parameters = self
            .source_audio_parameters
            .as_ref()
            .or(new_snapshot.as_ref())
            .expect("source audio parameters initialized");
        self.audio = Some(unsafe {
            AacTranscoder::new(
                source_index,
                output_index,
                source_parameters.as_ptr(),
                next_pts,
            )?
        });
        if let Some(snapshot) = new_snapshot {
            self.source_audio_parameters = Some(snapshot);
        }
        self.recovery_frames = 0;
        Ok(true)
    }

    pub fn note_real_audio(&mut self, master_clock_us: i64) {
        self.last_real_audio_us = Some(master_clock_us);
    }

    pub fn degrade_to_silence(&mut self) {
        self.audio = None;
        self.last_real_audio_us = None;
        self.recovery_frames = 0;
    }

    pub fn disable_audio(&mut self) {
        self.audio = None;
        self.silent = None;
        self.last_real_audio_us = None;
        self.recovery_frames = 0;
    }

    pub fn silence_until(&mut self, master_clock_us: i64) -> GlobalResult<Vec<OwnedPacket>> {
        if self.audio.is_some()
            && self
                .last_real_audio_us
                .is_some_and(|last| master_clock_us.saturating_sub(last) > AUDIO_STALL_GRACE_US)
        {
            self.degrade_to_silence();
        }
        if self.audio.is_some() {
            return Ok(Vec::new());
        }
        match self.silent.as_mut() {
            Some(silent) => silent.emit_until(master_clock_us),
            None => Ok(Vec::new()),
        }
    }

    pub unsafe fn process(&mut self, packet: &AVPacket) -> GlobalResult<Vec<OwnedPacket>> {
        match self.audio.as_mut() {
            Some(audio) if audio.stream_index == packet.stream_index => unsafe {
                audio.process(packet)
            },
            _ => Ok(Vec::new()),
        }
    }

    pub fn flush(&mut self) -> GlobalResult<Vec<OwnedPacket>> {
        match self.audio.as_mut() {
            Some(audio) => audio.flush(),
            None => Ok(Vec::new()),
        }
    }
}

fn supported_audio_source(codec_id: rsmpeg::ffi::AVCodecID) -> bool {
    matches!(
        codec_id,
        AVCodecID_AV_CODEC_ID_AAC
            | AVCodecID_AV_CODEC_ID_PCM_ALAW
            | AVCodecID_AV_CODEC_ID_PCM_MULAW
            | AVCodecID_AV_CODEC_ID_G723_1
            | AVCodecID_AV_CODEC_ID_G729
            | AVCodecID_AV_CODEC_ID_MP2
    ) && AVCodec::find_decoder(codec_id).is_some()
}

struct SilentAacSource {
    encoder: AVCodecContext,
    stream_index: i32,
    next_frame_pts: i64,
    next_packet_pts: i64,
}

fn silent_aac_or_warn(stream_index: i32) -> Option<SilentAacSource> {
    match SilentAacSource::new(stream_index) {
        Ok(source) => Some(source),
        Err(error) => {
            warn!(
                "silent AAC source unavailable: action=audio_placeholder, outcome=audio_disabled, stream_index={stream_index}, reason={error}"
            );
            None
        }
    }
}

impl SilentAacSource {
    fn new(stream_index: i32) -> GlobalResult<Self> {
        let encoder_codec = AVCodec::find_encoder(AVCodecID_AV_CODEC_ID_AAC).ok_or_else(|| {
            transcode_error(
                "SILENT_AAC_INIT_FAILED",
                "AAC encoder is unavailable".to_string(),
            )
        })?;
        let sample_fmt = encoder_codec
            .sample_fmts()
            .and_then(|formats| formats.first().copied())
            .ok_or_else(|| {
                transcode_error(
                    "SILENT_AAC_INIT_FAILED",
                    "AAC encoder exposes no sample format".to_string(),
                )
            })?;
        let mut encoder = AVCodecContext::new(&encoder_codec);
        encoder.set_bit_rate(AAC_BIT_RATE);
        encoder.set_sample_rate(AAC_SAMPLE_RATE);
        encoder.set_channel_layout(av_get_default_channel_layout(AAC_CHANNELS) as u64);
        encoder.set_channels(AAC_CHANNELS);
        encoder.set_sample_fmt(sample_fmt);
        encoder.set_time_base(AVRational {
            num: 1,
            den: AAC_SAMPLE_RATE,
        });
        unsafe {
            (*encoder.as_mut_ptr()).profile = FF_PROFILE_AAC_LOW as i32;
            (*encoder.as_mut_ptr()).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }
        encoder.open(None).map_err(|error| {
            transcode_error(
                "SILENT_AAC_INIT_FAILED",
                format!("open AAC encoder failed: {error}"),
            )
        })?;
        let mut source = Self {
            encoder,
            stream_index,
            next_frame_pts: -2048,
            next_packet_pts: 0,
        };
        for _ in 0..2 {
            drop(source.encode_silent_frame()?);
        }
        source.next_packet_pts = 0;
        Ok(source)
    }

    fn emit_until(&mut self, master_clock_us: i64) -> GlobalResult<Vec<OwnedPacket>> {
        let target_pts = master_clock_us
            .max(0)
            .saturating_mul(i64::from(AAC_SAMPLE_RATE))
            / 1_000_000;
        if target_pts.saturating_sub(self.next_packet_pts) > 12_000 {
            self.next_packet_pts = target_pts - target_pts.rem_euclid(1024);
            self.next_frame_pts = self.next_packet_pts;
        }
        let mut output = Vec::new();
        for _ in 0..4 {
            if self.next_packet_pts > target_pts.saturating_add(1024) {
                break;
            }
            let packets = self.encode_silent_frame()?;
            for mut packet in packets {
                unsafe {
                    (*packet.as_mut_ptr()).pts = self.next_packet_pts;
                    (*packet.as_mut_ptr()).dts = self.next_packet_pts;
                    (*packet.as_mut_ptr()).duration = 1024;
                }
                packet.set_stream_index(self.stream_index);
                packet.set_pos(-1);
                self.next_packet_pts = self.next_packet_pts.saturating_add(1024);
                output.push(packet);
            }
        }
        Ok(output)
    }

    fn encode_silent_frame(&mut self) -> GlobalResult<Vec<OwnedPacket>> {
        let frame_size = self.encoder.frame_size.max(1);
        let mut frame = AVFrame::new();
        frame.set_format(self.encoder.sample_fmt);
        frame.set_sample_rate(AAC_SAMPLE_RATE);
        frame.set_channel_layout(self.encoder.channel_layout);
        frame.set_nb_samples(frame_size);
        frame.set_pts(self.next_frame_pts);
        frame.alloc_buffer().map_err(|error| {
            transcode_error(
                "SILENT_AAC_FAILED",
                format!("allocate silent AAC frame failed: {error}"),
            )
        })?;
        let ret = unsafe {
            av_samples_set_silence(
                frame.extended_data,
                0,
                frame_size,
                AAC_CHANNELS,
                self.encoder.sample_fmt,
            )
        };
        if ret < 0 {
            return Err(transcode_error(
                "SILENT_AAC_FAILED",
                format!("initialize silent AAC frame failed: {ret}"),
            ));
        }
        self.next_frame_pts = self.next_frame_pts.saturating_add(i64::from(frame_size));
        self.encoder.send_frame(Some(&frame)).map_err(|error| {
            transcode_error(
                "SILENT_AAC_FAILED",
                format!("send silent AAC frame failed: {error}"),
            )
        })?;
        let mut output = Vec::new();
        loop {
            match self.encoder.receive_packet() {
                Ok(packet) => output.push(packet),
                Err(RsmpegError::EncoderDrainError | RsmpegError::EncoderFlushedError) => break,
                Err(error) => {
                    return Err(transcode_error(
                        "SILENT_AAC_FAILED",
                        format!("receive silent AAC packet failed: {error}"),
                    ));
                }
            }
        }
        Ok(output)
    }
}

struct CodecParametersSnapshot {
    parameters: *mut rsmpeg::ffi::AVCodecParameters,
}

impl CodecParametersSnapshot {
    unsafe fn copy(source: *const rsmpeg::ffi::AVCodecParameters) -> GlobalResult<Self> {
        let parameters = unsafe { avcodec_parameters_alloc() };
        if parameters.is_null() {
            return Err(transcode_error(
                "AUDIO_TRANSCODE_INIT_FAILED",
                "allocate source codec parameter snapshot failed".to_string(),
            ));
        }
        let ret = unsafe { avcodec_parameters_copy(parameters, source) };
        if ret < 0 {
            let mut parameters = parameters;
            unsafe { avcodec_parameters_free(&mut parameters) };
            return Err(transcode_error(
                "AUDIO_TRANSCODE_INIT_FAILED",
                format!("copy source codec parameters failed: {ret}"),
            ));
        }
        Ok(Self { parameters })
    }

    fn as_ptr(&self) -> *const rsmpeg::ffi::AVCodecParameters {
        self.parameters
    }
}

impl Drop for CodecParametersSnapshot {
    fn drop(&mut self) {
        unsafe { avcodec_parameters_free(&mut self.parameters) };
    }
}

struct AacTranscoder {
    stream_index: i32,
    output_stream_index: i32,
    decoder: AVCodecContext,
    encoder: AVCodecContext,
    resampler: SwrContext,
    fifo: AVAudioFifo,
    next_pts: i64,
    flushed: bool,
}

impl AacTranscoder {
    unsafe fn new(
        source_stream_index: i32,
        output_stream_index: i32,
        source_parameters: *const rsmpeg::ffi::AVCodecParameters,
        next_pts: i64,
    ) -> GlobalResult<Self> {
        unsafe {
            let codecpar = source_parameters;
            let decoder_codec = AVCodec::find_decoder((*codecpar).codec_id).ok_or_else(|| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    format!(
                        "audio decoder is unavailable for codec id {}",
                        (*codecpar).codec_id
                    ),
                )
            })?;
            let mut decoder = AVCodecContext::new(&decoder_codec);
            let ret = avcodec_parameters_to_context(decoder.as_mut_ptr(), codecpar);
            if ret < 0 {
                return Err(transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    format!("copy decoder parameters failed: {ret}"),
                ));
            }
            decoder.open(None).map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    format!("open audio decoder failed: {err}"),
                )
            })?;

            let encoder_codec =
                AVCodec::find_encoder(AVCodecID_AV_CODEC_ID_AAC).ok_or_else(|| {
                    transcode_error(
                        "AUDIO_TRANSCODE_INIT_FAILED",
                        "AAC encoder is unavailable".to_string(),
                    )
                })?;
            let sample_fmt = encoder_codec
                .sample_fmts()
                .and_then(|formats| formats.first().copied())
                .ok_or_else(|| {
                    transcode_error(
                        "AUDIO_TRANSCODE_INIT_FAILED",
                        "AAC encoder exposes no sample format".to_string(),
                    )
                })?;
            let mono_layout = av_get_default_channel_layout(AAC_CHANNELS) as u64;
            let mut encoder = AVCodecContext::new(&encoder_codec);
            encoder.set_bit_rate(AAC_BIT_RATE);
            encoder.set_sample_rate(AAC_SAMPLE_RATE);
            encoder.set_channel_layout(mono_layout);
            encoder.set_channels(AAC_CHANNELS);
            encoder.set_sample_fmt(sample_fmt);
            encoder.set_time_base(AVRational {
                num: 1,
                den: AAC_SAMPLE_RATE,
            });
            (*encoder.as_mut_ptr()).profile = FF_PROFILE_AAC_LOW as i32;
            (*encoder.as_mut_ptr()).flags |= AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            encoder.open(None).map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    format!("open AAC encoder failed: {err}"),
                )
            })?;

            let input_rate = if decoder.sample_rate > 0 {
                decoder.sample_rate
            } else {
                8_000
            };
            let input_channels = if decoder.channels > 0 {
                decoder.channels
            } else {
                1
            };
            let input_layout = if decoder.channel_layout != 0 {
                decoder.channel_layout
            } else {
                av_get_default_channel_layout(input_channels) as u64
            };
            let mut resampler = SwrContext::new(
                mono_layout,
                sample_fmt,
                AAC_SAMPLE_RATE,
                input_layout,
                decoder.sample_fmt,
                input_rate,
            )
            .ok_or_else(|| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    "allocate audio resampler failed".to_string(),
                )
            })?;
            resampler.init().map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    format!("initialize audio resampler failed: {err}"),
                )
            })?;

            Ok(Self {
                stream_index: source_stream_index,
                output_stream_index,
                decoder,
                encoder,
                resampler,
                fifo: AVAudioFifo::new(sample_fmt, AAC_CHANNELS, 1),
                next_pts,
                flushed: false,
            })
        }
    }

    unsafe fn process(&mut self, packet: &AVPacket) -> GlobalResult<Vec<OwnedPacket>> {
        unsafe {
            let mut owned = OwnedPacket::new();
            if av_packet_ref(owned.as_mut_ptr(), packet) < 0 {
                return Err(transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    "reference input audio packet failed".to_string(),
                ));
            }
            self.decoder.send_packet(Some(&owned)).map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("send audio packet failed: {err}"),
                )
            })?;
            let mut output = Vec::new();
            self.drain_decoder(&mut output)?;
            Ok(output)
        }
    }

    fn flush(&mut self) -> GlobalResult<Vec<OwnedPacket>> {
        if self.flushed {
            return Ok(Vec::new());
        }
        self.flushed = true;
        let mut output = Vec::new();
        match self.decoder.send_packet(None) {
            Ok(()) | Err(RsmpegError::DecoderFlushedError) => {}
            Err(err) => {
                return Err(transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("flush audio decoder failed: {err}"),
                ));
            }
        }
        self.drain_decoder(&mut output)?;

        let mut delayed = AVFrame::new();
        delayed.set_format(self.encoder.sample_fmt);
        delayed.set_sample_rate(AAC_SAMPLE_RATE);
        delayed.set_channel_layout(self.encoder.channel_layout);
        self.resampler
            .convert_frame(None, &mut delayed)
            .map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("flush audio resampler failed: {err}"),
                )
            })?;
        if delayed.nb_samples > 0 {
            unsafe {
                self.fifo
                    .write(delayed.extended_data, delayed.nb_samples)
                    .map_err(|err| {
                        transcode_error(
                            "AUDIO_TRANSCODE_FAILED",
                            format!("write delayed audio samples failed: {err}"),
                        )
                    })?;
            }
        }
        self.encode_available(true, &mut output)?;
        match self.encoder.send_frame(None) {
            Ok(()) | Err(RsmpegError::EncoderFlushedError) => {}
            Err(err) => {
                return Err(transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("flush AAC encoder failed: {err}"),
                ));
            }
        }
        self.drain_encoder(&mut output)?;
        Ok(output)
    }

    fn drain_decoder(&mut self, output: &mut Vec<OwnedPacket>) -> GlobalResult<()> {
        loop {
            match self.decoder.receive_frame() {
                Ok(frame) => self.convert_frame(&frame, output)?,
                Err(RsmpegError::DecoderDrainError | RsmpegError::DecoderFlushedError) => break,
                Err(err) => {
                    return Err(transcode_error(
                        "AUDIO_TRANSCODE_FAILED",
                        format!("decode audio frame failed: {err}"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn convert_frame(
        &mut self,
        input: &AVFrame,
        output: &mut Vec<OwnedPacket>,
    ) -> GlobalResult<()> {
        let mut converted = AVFrame::new();
        converted.set_format(self.encoder.sample_fmt);
        converted.set_sample_rate(AAC_SAMPLE_RATE);
        converted.set_channel_layout(self.encoder.channel_layout);
        self.resampler
            .convert_frame(Some(input), &mut converted)
            .map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("resample audio frame failed: {err}"),
                )
            })?;
        if converted.nb_samples > 0 {
            unsafe {
                self.fifo
                    .write(converted.extended_data, converted.nb_samples)
                    .map_err(|err| {
                        transcode_error(
                            "AUDIO_TRANSCODE_FAILED",
                            format!("write audio FIFO failed: {err}"),
                        )
                    })?;
            }
        }
        self.encode_available(false, output)
    }

    fn encode_available(
        &mut self,
        flush_partial: bool,
        output: &mut Vec<OwnedPacket>,
    ) -> GlobalResult<()> {
        let frame_size = self.encoder.frame_size.max(1);
        while self.fifo.size() >= frame_size || (flush_partial && self.fifo.size() > 0) {
            let available = self.fifo.size().min(frame_size);
            let mut frame = AVFrame::new();
            frame.set_format(self.encoder.sample_fmt);
            frame.set_sample_rate(AAC_SAMPLE_RATE);
            frame.set_channel_layout(self.encoder.channel_layout);
            frame.set_nb_samples(frame_size);
            frame.set_pts(self.next_pts);
            frame.alloc_buffer().map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("allocate AAC frame failed: {err}"),
                )
            })?;
            unsafe {
                self.fifo
                    .read(frame.extended_data, available)
                    .map_err(|err| {
                        transcode_error(
                            "AUDIO_TRANSCODE_FAILED",
                            format!("read audio FIFO failed: {err}"),
                        )
                    })?;
                if available < frame_size {
                    let ret = av_samples_set_silence(
                        frame.extended_data,
                        available,
                        frame_size - available,
                        AAC_CHANNELS,
                        self.encoder.sample_fmt,
                    );
                    if ret < 0 {
                        return Err(transcode_error(
                            "AUDIO_TRANSCODE_FAILED",
                            format!("pad final AAC frame failed: {ret}"),
                        ));
                    }
                }
            }
            self.next_pts += i64::from(frame_size);
            self.encoder.send_frame(Some(&frame)).map_err(|err| {
                transcode_error(
                    "AUDIO_TRANSCODE_FAILED",
                    format!("send AAC frame failed: {err}"),
                )
            })?;
            self.drain_encoder(output)?;
        }
        Ok(())
    }

    fn drain_encoder(&mut self, output: &mut Vec<OwnedPacket>) -> GlobalResult<()> {
        loop {
            match self.encoder.receive_packet() {
                Ok(mut packet) => {
                    packet.set_stream_index(self.output_stream_index);
                    packet.set_pos(-1);
                    output.push(packet);
                }
                Err(RsmpegError::EncoderDrainError | RsmpegError::EncoderFlushedError) => break,
                Err(err) => {
                    return Err(transcode_error(
                        "AUDIO_TRANSCODE_FAILED",
                        format!("receive AAC packet failed: {err}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn unsupported(code: &str, detail: String) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::Unsupported.code(),
        &format!("{code}: {detail}"),
        |message| error!("{message}"),
    )
}

fn transcode_error(code: &str, detail: String) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        &format!("{code}: {detail}"),
        |message| error!("{message}"),
    )
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use base::tokio_util::sync::CancellationToken;
    use gmv_domain::info::media_info::{OutputAudioCodec, TranscodeConfig};
    use rsmpeg::ffi::{
        AVMediaType_AVMEDIA_TYPE_AUDIO, AVSampleFormat_AV_SAMPLE_FMT_S16,
        av_channel_layout_default, av_new_packet, av_packet_unref, avformat_alloc_context,
        avformat_new_stream,
    };

    use super::*;
    use crate::media::context::format::demuxer::{AvioResource, ParamRepairState};
    use crate::media::rtp::RtpReadControl;

    #[test]
    fn transcoded_audio_recovers_without_mutating_source_parameters() {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            assert!(!fmt_ctx.is_null());
            let stream = avformat_new_stream(fmt_ctx, ptr::null());
            assert!(!stream.is_null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_PCM_ALAW;
            (*codecpar).sample_rate = 8_000;
            (*codecpar).channels = 1;
            (*codecpar).channel_layout = 4;
            (*codecpar).bits_per_coded_sample = 8;
            (*codecpar).block_align = 1;
            (*stream).time_base = AVRational { num: 1, den: 8_000 };

            let mut demuxer = DemuxerContext {
                avio: AvioResource {
                    fmt_ctx,
                    io_buf: ptr::null_mut(),
                    avio_ctx: ptr::null_mut(),
                },
                params: vec![ParamRepairState {
                    ready: true,
                    ..Default::default()
                }],
                read_control: Arc::new(RtpReadControl::new(
                    CancellationToken::new(),
                    Instant::now() + Duration::from_secs(60),
                )),
                output_plan: Default::default(),
            };
            let mut codec = CodecContext::init(
                None,
                Some(TranscodeConfig {
                    audio_codec: Some(OutputAudioCodec::Aac),
                }),
            )
            .unwrap();
            codec.prepare(&mut demuxer, true, &[true]).unwrap();
            assert_eq!((*codecpar).codec_id, AVCodecID_AV_CODEC_ID_PCM_ALAW);
            assert_eq!(codec.transcoded_stream_index(), Some(0));
            assert!(codec.has_silent_audio());

            codec.degrade_to_silence();
            for _ in 0..AUDIO_RECOVERY_FRAMES {
                codec.observe_ready_audio(&mut demuxer, 0, 7).unwrap();
            }
            assert_eq!(codec.audio.as_ref().unwrap().output_stream_index, 7);
            assert_eq!((*codecpar).codec_id, AVCodecID_AV_CODEC_ID_PCM_ALAW);
        }
    }

    #[test]
    fn unsupported_siren_uses_silence_without_rejecting_video_pipeline() {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            let stream = avformat_new_stream(fmt_ctx, ptr::null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = rsmpeg::ffi::AVCodecID_AV_CODEC_ID_SIREN;
            (*codecpar).sample_rate = 16_000;
            (*codecpar).channels = 1;
            av_channel_layout_default(&mut (*codecpar).ch_layout, 1);
            let mut demuxer = DemuxerContext {
                avio: AvioResource {
                    fmt_ctx,
                    io_buf: ptr::null_mut(),
                    avio_ctx: ptr::null_mut(),
                },
                params: vec![ParamRepairState::default()],
                read_control: Arc::new(RtpReadControl::new(
                    CancellationToken::new(),
                    Instant::now() + Duration::from_secs(60),
                )),
                output_plan: Default::default(),
            };
            let mut codec = CodecContext::fixed_aac();

            codec.prepare(&mut demuxer, true, &[false]).unwrap();

            assert_eq!(codec.rejected_audio_stream(), Some(0));
            assert!(codec.has_silent_audio());
            assert!(!codec.has_real_audio());
            for _ in 0..AUDIO_RECOVERY_FRAMES {
                assert!(!codec.observe_ready_audio(&mut demuxer, 0, 0).unwrap());
            }
            assert!(!codec.has_real_audio());
        }
    }

    #[test]
    fn mp2_audio_prepares_real_aac_transcoder() {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            let stream = avformat_new_stream(fmt_ctx, ptr::null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_MP2;
            (*codecpar).sample_rate = 48_000;
            (*codecpar).channels = 2;
            (*codecpar).bit_rate = 128_000;
            (*codecpar).format = AVSampleFormat_AV_SAMPLE_FMT_S16 as i32;
            (*codecpar).frame_size = 1_152;
            av_channel_layout_default(&mut (*codecpar).ch_layout, 2);
            (*stream).time_base = AVRational {
                num: 1,
                den: 48_000,
            };
            let mut demuxer = DemuxerContext {
                avio: AvioResource {
                    fmt_ctx,
                    io_buf: ptr::null_mut(),
                    avio_ctx: ptr::null_mut(),
                },
                params: vec![ParamRepairState {
                    ready: true,
                    ..Default::default()
                }],
                read_control: Arc::new(RtpReadControl::new(
                    CancellationToken::new(),
                    Instant::now() + Duration::from_secs(60),
                )),
                output_plan: Default::default(),
            };
            let mut codec = CodecContext::fixed_aac();

            codec.prepare(&mut demuxer, true, &[true]).unwrap();

            assert_eq!(codec.rejected_audio_stream(), None);
            assert_eq!(codec.transcoded_stream_index(), Some(0));
            assert!(codec.has_real_audio());
            assert!(codec.has_silent_audio());
        }
    }

    unsafe fn transcode_fixture(
        codec_id: rsmpeg::ffi::AVCodecID,
        frame: &[u8],
        frame_count: usize,
        bit_rate: i64,
    ) -> usize {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            let stream = avformat_new_stream(fmt_ctx, ptr::null());
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = codec_id;
            (*codecpar).sample_rate = 8_000;
            (*codecpar).channels = 1;
            (*codecpar).channel_layout = 4;
            av_channel_layout_default(&mut (*codecpar).ch_layout, 1);
            (*codecpar).block_align = frame.len() as i32;
            (*codecpar).bit_rate = bit_rate;
            let mut transcoder = AacTranscoder::new(0, 0, codecpar, 0).unwrap();
            let mut output_count = 0;
            for _ in 0..frame_count {
                let mut packet = std::mem::zeroed::<AVPacket>();
                assert_eq!(av_new_packet(&mut packet, frame.len() as i32), 0);
                ptr::copy_nonoverlapping(frame.as_ptr(), packet.data, frame.len());
                output_count += transcoder.process(&packet).unwrap().len();
                av_packet_unref(&mut packet);
            }
            output_count += transcoder.flush().unwrap().len();
            rsmpeg::ffi::avformat_free_context(fmt_ctx);
            output_count
        }
    }

    #[test]
    fn promised_gb_audio_packets_decode_resample_and_encode_to_aac() {
        unsafe {
            let fixtures: [(rsmpeg::ffi::AVCodecID, Vec<u8>, usize, i64); 4] = [
                (AVCodecID_AV_CODEC_ID_PCM_ALAW, vec![0xd5; 160], 8, 64_000),
                (AVCodecID_AV_CODEC_ID_PCM_MULAW, vec![0xff; 160], 8, 64_000),
                (AVCodecID_AV_CODEC_ID_G723_1, vec![0; 24], 6, 6_300),
                (AVCodecID_AV_CODEC_ID_G729, vec![0; 10], 16, 8_000),
            ];
            for (codec_id, frame, frame_count, bit_rate) in fixtures {
                assert!(
                    transcode_fixture(codec_id, &frame, frame_count, bit_rate) > 0,
                    "codec id {codec_id} produced no AAC packets"
                );
            }
        }
    }
}
