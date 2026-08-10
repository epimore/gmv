use crate::media::context::format::demuxer::DemuxerContext;
use crate::state::layer::codec_layer::CodecLayer;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use gmv_domain::info::media_info::{OutputAudioCodec, TranscodeConfig};
use rsmpeg::avcodec::{AVCodec, AVCodecContext, AVPacket as OwnedPacket};
use rsmpeg::avutil::{AVAudioFifo, AVFrame, AVRational, av_get_default_channel_layout};
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi::{
    AV_CODEC_FLAG_GLOBAL_HEADER, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264,
    AVCodecID_AV_CODEC_ID_HEVC, AVCodecID_AV_CODEC_ID_PCM_ALAW, AVCodecID_AV_CODEC_ID_PCM_MULAW,
    AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, FF_PROFILE_AAC_LOW,
    av_packet_ref, av_samples_set_silence, avcodec_parameters_from_context,
    avcodec_parameters_to_context,
};
use rsmpeg::swresample::SwrContext;

const AAC_SAMPLE_RATE: i32 = 48_000;
const AAC_CHANNELS: i32 = 1;
const AAC_BIT_RATE: i64 = 48_000;

pub struct CodecContext {
    target_audio: Option<OutputAudioCodec>,
    audio: Option<AacTranscoder>,
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
        })
    }

    pub unsafe fn prepare(&mut self, demuxer: &mut DemuxerContext) -> GlobalResult<()> {
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
                        if audio_stream.is_none() {
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
                return Ok(());
            };
            let codecpar = (*stream).codecpar;
            match (*codecpar).codec_id {
                AVCodecID_AV_CODEC_ID_AAC => Ok(()),
                AVCodecID_AV_CODEC_ID_PCM_ALAW | AVCodecID_AV_CODEC_ID_PCM_MULAW => {
                    self.audio = Some(AacTranscoder::new(stream_index, stream)?);
                    Ok(())
                }
                codec_id => Err(unsupported(
                    "UNSUPPORTED_AUDIO_SOURCE_CODEC",
                    format!("unsupported audio codec id {codec_id}"),
                )),
            }
        }
    }

    pub unsafe fn refresh_output_parameters(
        &self,
        demuxer: &mut DemuxerContext,
    ) -> GlobalResult<()> {
        let Some(audio) = self.audio.as_ref() else {
            return Ok(());
        };
        unsafe {
            let fmt_ctx = demuxer.avio.fmt_ctx;
            let stream = *(*fmt_ctx).streams.add(audio.stream_index as usize);
            export_aac_parameters(stream, &audio.encoder)
        }
    }

    pub fn handles(&self, packet: &AVPacket) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.stream_index == packet.stream_index)
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

struct AacTranscoder {
    stream_index: i32,
    decoder: AVCodecContext,
    encoder: AVCodecContext,
    resampler: SwrContext,
    fifo: AVAudioFifo,
    next_pts: i64,
    flushed: bool,
}

impl AacTranscoder {
    unsafe fn new(stream_index: i32, stream: *mut rsmpeg::ffi::AVStream) -> GlobalResult<Self> {
        unsafe {
            let codecpar = (*stream).codecpar;
            let decoder_codec = AVCodec::find_decoder((*codecpar).codec_id).ok_or_else(|| {
                transcode_error(
                    "AUDIO_TRANSCODE_INIT_FAILED",
                    "G.711 decoder is unavailable".to_string(),
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
                    format!("open G.711 decoder failed: {err}"),
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

            export_aac_parameters(stream, &encoder)?;

            Ok(Self {
                stream_index,
                decoder,
                encoder,
                resampler,
                fifo: AVAudioFifo::new(sample_fmt, AAC_CHANNELS, 1),
                next_pts: 0,
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
                    format!("send G.711 packet failed: {err}"),
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
                    format!("flush G.711 decoder failed: {err}"),
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
                        format!("decode G.711 frame failed: {err}"),
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
                    format!("resample G.711 frame failed: {err}"),
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
                    packet.set_stream_index(self.stream_index);
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

unsafe fn export_aac_parameters(
    stream: *mut rsmpeg::ffi::AVStream,
    encoder: &AVCodecContext,
) -> GlobalResult<()> {
    unsafe {
        let codecpar = (*stream).codecpar;
        let ret = avcodec_parameters_from_context(codecpar, encoder.as_ptr());
        if ret < 0 {
            return Err(transcode_error(
                "AUDIO_TRANSCODE_INIT_FAILED",
                format!("export AAC parameters failed: {ret}"),
            ));
        }
        (*codecpar).codec_tag = 0;
        (*stream).time_base = AVRational {
            num: 1,
            den: AAC_SAMPLE_RATE,
        };
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

    use gmv_domain::info::media_info::{OutputAudioCodec, TranscodeConfig};
    use rsmpeg::ffi::{
        AVMediaType_AVMEDIA_TYPE_AUDIO, avformat_alloc_context, avformat_new_stream,
    };

    use super::*;
    use crate::media::context::format::demuxer::{AvioResource, ParamRepairState};

    #[test]
    fn refresh_output_parameters_restores_aac_after_demuxer_codec_update() {
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
                params: Vec::<ParamRepairState>::new(),
            };
            let mut codec = CodecContext::init(
                None,
                Some(TranscodeConfig {
                    audio_codec: Some(OutputAudioCodec::Aac),
                }),
            )
            .unwrap();
            codec.prepare(&mut demuxer).unwrap();
            assert_eq!((*codecpar).codec_id, AVCodecID_AV_CODEC_ID_AAC);

            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_PCM_ALAW;
            codec.refresh_output_parameters(&mut demuxer).unwrap();

            assert_eq!((*codecpar).codec_id, AVCodecID_AV_CODEC_ID_AAC);
            assert_eq!((*stream).time_base.den, AAC_SAMPLE_RATE);
        }
    }
}
