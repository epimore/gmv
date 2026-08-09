use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::fmp4::clone_packet_for_mp4;
use crate::media::context::format::{
    FmtMuxer, HlsPart, MuxPacket, MuxPacketSender, can_start_fragmented_output, write_callback,
};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::warn;
use base::once_cell::sync::Lazy;
use log::error;
use rsmpeg::ffi::{
    AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AVFMT_FLAG_AUTO_BSF, AVFMT_FLAG_FLUSH_PACKETS, AVFMT_NOFILE,
    AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational,
    AVStream, av_dict_set, av_free, av_guess_format, av_interleaved_write_frame, av_malloc,
    av_packet_rescale_ts, av_packet_unref, av_rescale_q, av_write_frame, av_write_trailer,
    avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header,
    avio_alloc_context, avio_context_free, avio_flush,
};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use std::collections::HashMap;
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::time::Instant;

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());
pub const HLS_PART_TARGET_US: u64 = 500_000;
const HLS_PART_FRAGMENT_US: u64 = 425_000;
pub const HLS_SEGMENT_TARGET_US: u64 = 2_000_000;
const HLS_SEGMENT_MAX_US: u64 = 3_500_000;
pub struct HlsFmp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: MuxPacketSender,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    in_timebase_map: HashMap<c_int, AVRational>,
    v_idx: c_int,
    started: bool,
    fragment_started_with_key: bool,
    fragment_start_us: i64,
    fragment_end_us: i64,
    segment_start_us: i64,
    segment_seq: usize,
    part_seq: usize,
    flushed: bool,
    init_sent: bool,
    pub epoch: Instant,
    pub seq: usize,
}
impl Drop for HlsFmp4Context {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                rsmpeg::ffi::avformat_free_context(self.fmt_ctx);
            }
            if !self.avio_ctx.is_null() {
                avio_context_free(&mut self.avio_ctx);
            }
            self.io_buf = ptr::null_mut();

            if !self.out_buf_ptr.is_null() {
                drop(Box::from_raw(self.out_buf_ptr));
                self.out_buf_ptr = ptr::null_mut();
            }
        }
    }
}
impl FmtMuxer for HlsFmp4Context {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: MuxPacketSender,
    ) -> GlobalResult<Self> {
        unsafe {
            let io_buf = av_malloc(DEFAULT_IO_BUF_SIZE) as *mut u8;
            if io_buf.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate IO buffer",
                    |msg| warn!("{msg}"),
                ));
            }

            let out_vec = Box::new(Vec::<u8>::new());
            let out_buf_ptr = Box::into_raw(out_vec);

            let avio_ctx = avio_alloc_context(
                io_buf,
                DEFAULT_IO_BUF_SIZE as c_int,
                1,
                out_buf_ptr as *mut c_void,
                None,
                Some(write_callback),
                None,
            );
            if avio_ctx.is_null() {
                av_free(io_buf as *mut c_void);
                drop(Box::from_raw(out_buf_ptr));
                return Err(GlobalError::new_sys_error(
                    "Failed to allocate AVIO context",
                    |msg| warn!("{msg}"),
                ));
            }

            let out_fmt_ctx = avformat_alloc_context();
            (*out_fmt_ctx).pb = avio_ctx;
            (*out_fmt_ctx).oformat = av_guess_format(MP4.as_ptr(), ptr::null(), ptr::null());
            (*out_fmt_ctx).max_delay = 100_000;
            (*out_fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            (*out_fmt_ctx).flags |= AVFMT_NOFILE as i32;
            (*out_fmt_ctx).flags |= AVFMT_FLAG_AUTO_BSF as i32;
            if (*out_fmt_ctx).oformat.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }

            // === CMAF flags ===
            // 创建AVDictionary
            let mut options = ptr::null_mut::<rsmpeg::ffi::AVDictionary>();

            let movflags = CString::new("frag_keyframe+empty_moov+default_base_moof").unwrap();
            rsmpeg::ffi::av_dict_set(
                &mut options,
                CString::new("movflags").unwrap().as_ptr(),
                movflags.as_ptr(),
                0,
            );
            let frag_duration = CString::new(HLS_PART_FRAGMENT_US.to_string()).unwrap();
            av_dict_set(
                &mut options,
                CString::new("frag_duration").unwrap().as_ptr(),
                frag_duration.as_ptr(),
                0,
            );
            let mut in_timebase_map = HashMap::with_capacity(8);
            let in_fmt_ctx = demuxer_context.avio.fmt_ctx;
            let v_idx = copy_streams(&mut in_timebase_map, in_fmt_ctx, out_fmt_ctx)?;

            let ret = avformat_write_header(out_fmt_ctx, &mut options);
            // 释放选项字典
            if !options.is_null() {
                rsmpeg::ffi::av_dict_free(&mut options);
            }
            if ret < 0 {
                return Err(GlobalError::new_sys_error(
                    &format!("FMP4 header write failed: {}", show_ffmpeg_error_msg(ret)),
                    |msg| error!("{msg}"),
                ));
            }

            // === init segment ===
            let init_data = {
                let buf = &mut *out_buf_ptr;
                Bytes::from(std::mem::take(buf))
            };

            Ok(Self {
                init_segment: init_data,
                pkt_tx,
                fmt_ctx: out_fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf_ptr,
                in_timebase_map,
                v_idx,
                started: false,
                fragment_started_with_key: false,
                fragment_start_us: 0,
                fragment_end_us: 0,
                segment_start_us: 0,
                segment_seq: 0,
                part_seq: 0,
                flushed: false,
                init_sent: false,
                epoch: Instant::now(),
                seq: 0,
            })
        }
    }
    fn get_header(&self) -> Bytes {
        self.init_segment.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, _timestamp: u64) -> GlobalResult<()> {
        unsafe {
            match self.in_timebase_map.get(&pkt.stream_index) {
                None => {
                    warn!(
                        "fMP4 write failed,stream index error: {}",
                        &pkt.stream_index
                    );
                    return Ok(());
                }
                Some(&in_tb) => {
                    let packet_time_us = packet_time_us(pkt, in_tb);
                    let packet_end_us = packet_end_us(pkt, in_tb, packet_time_us);
                    let is_keyframe =
                        self.v_idx == pkt.stream_index && (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0;
                    if !can_start_fragmented_output(
                        self.started,
                        self.v_idx,
                        pkt.stream_index,
                        is_keyframe,
                    ) {
                        return Ok(());
                    }
                    let segment_elapsed_us =
                        packet_time_us.saturating_sub(self.segment_start_us).max(0) as u64;
                    let segment_boundary = self.started
                        && ((segment_elapsed_us >= HLS_SEGMENT_TARGET_US
                            && (self.v_idx < 0 || is_keyframe))
                            || segment_elapsed_us >= HLS_SEGMENT_MAX_US);
                    if !self.started {
                        self.started = true;
                        self.fragment_started_with_key = self.v_idx < 0 || is_keyframe;
                        self.fragment_start_us = packet_time_us;
                        self.fragment_end_us = packet_end_us;
                        self.segment_start_us = packet_time_us;
                    }
                    let out_st = *(*self.fmt_ctx).streams.add(pkt.stream_index as usize);
                    let codecpar = (*out_st).codecpar;
                    let strip_aac_adts = (*codecpar).codec_id == AVCodecID_AV_CODEC_ID_AAC
                        && !(*codecpar).extradata.is_null()
                        && (*codecpar).extradata_size > 0;
                    let mut cloned = clone_packet_for_mp4(pkt, strip_aac_adts)?;
                    av_packet_rescale_ts(cloned.as_mut_ptr(), in_tb, (*out_st).time_base);

                    (*cloned.as_mut_ptr()).pos = -1;
                    let ret = av_interleaved_write_frame(self.fmt_ctx, cloned.as_mut_ptr());
                    if ret < 0 {
                        return Err(GlobalError::new_sys_error(
                            &format!("HLS fMP4 write failed: {}", show_ffmpeg_error_msg(ret)),
                            |msg| warn!("{msg}"),
                        ));
                    }
                    self.fragment_end_us = self.fragment_end_us.max(packet_end_us);
                    if self.flush_fragment(
                        self.fragment_started_with_key,
                        packet_time_us,
                        segment_boundary,
                    ) {
                        if segment_boundary {
                            self.segment_seq += 1;
                            self.part_seq = 0;
                            self.segment_start_us = packet_time_us;
                        } else {
                            self.part_seq += 1;
                        }
                        self.fragment_started_with_key = is_keyframe;
                        self.fragment_start_us = packet_time_us;
                        self.fragment_end_us = packet_end_us;
                        // self.fragment_frame_count = 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn flush(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        unsafe {
            // 1. 写入所有缓冲帧
            av_write_frame(self.fmt_ctx, ptr::null_mut());

            // 2. 写入尾部并发送最后一个 part，同时完成父 segment
            av_write_trailer(self.fmt_ctx);
            avio_flush((*self.fmt_ctx).pb);
            self.flush_fragment(self.fragment_started_with_key, self.fragment_end_us, true);
        }
    }
}

impl HlsFmp4Context {
    pub fn next_segment_seq(&self) -> usize {
        self.segment_seq.saturating_add(1)
    }

    pub fn set_segment_seq(&mut self, segment_seq: usize) {
        self.segment_seq = segment_seq;
    }

    fn flush_fragment(&mut self, is_key: bool, boundary_us: i64, segment_complete: bool) -> bool {
        unsafe {
            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                return false;
            }
            let duration_us = boundary_us
                .saturating_sub(self.fragment_start_us)
                .max(self.fragment_end_us.saturating_sub(self.fragment_start_us))
                .max(1) as u64;
            self.seq += 1;
            let init_segment = if self.init_sent {
                None
            } else {
                self.init_sent = true;
                Some(self.init_segment.clone())
            };
            let data = Bytes::from(std::mem::take(out_vec));
            let _ = self.pkt_tx.send(Arc::new(MuxPacket {
                data,
                is_key,
                timestamp: (self.fragment_start_us.max(0) as u64) / 1_000_000,
                epoch: self.epoch,
                seq: self.seq,
                hls: Some(HlsPart {
                    segment_seq: self.segment_seq,
                    part_seq: self.part_seq,
                    duration_us,
                    segment_complete,
                    init_segment,
                }),
            }));
            true
        }
    }
}

fn packet_time_us(pkt: &AVPacket, time_base: AVRational) -> i64 {
    let timestamp = if pkt.dts != AV_NOPTS_VALUE {
        pkt.dts
    } else {
        pkt.pts
    };
    if timestamp == AV_NOPTS_VALUE {
        return 0;
    }
    unsafe {
        av_rescale_q(
            timestamp,
            time_base,
            AVRational {
                num: 1,
                den: 1_000_000,
            },
        )
    }
}

fn packet_end_us(pkt: &AVPacket, time_base: AVRational, start_us: i64) -> i64 {
    if pkt.duration <= 0 {
        return start_us;
    }
    let duration_us = unsafe {
        av_rescale_q(
            pkt.duration,
            time_base,
            AVRational {
                num: 1,
                den: 1_000_000,
            },
        )
    };
    start_us.saturating_add(duration_us.max(0))
}
pub fn copy_streams(
    base_time_map: &mut HashMap<i32, AVRational>,
    in_fmt_ctx: *mut rsmpeg::ffi::AVFormatContext,
    out_fmt_ctx: *mut rsmpeg::ffi::AVFormatContext,
) -> GlobalResult<c_int> {
    unsafe {
        let nb_streams = (*in_fmt_ctx).nb_streams;
        let mut v_idx = -1;

        for i in 0..nb_streams {
            let in_st = *(*in_fmt_ctx).streams.offset(i as isize);
            let codecpar = (*in_st).codecpar;

            // 只处理视频和音频流
            if !matches!(
                (*codecpar).codec_type,
                AVMediaType_AVMEDIA_TYPE_VIDEO | AVMediaType_AVMEDIA_TYPE_AUDIO
            ) {
                continue;
            }
            if (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
                v_idx = i as c_int;
            }

            // 创建输出流
            let out_st = avformat_new_stream(out_fmt_ctx, ptr::null_mut());
            if out_st.is_null() {
                return Err(GlobalError::new_sys_error(
                    "avformat_new_stream failed",
                    |msg| error!("msg"),
                ));
            }

            // 复制编解码器参数
            avcodec_parameters_copy((*out_st).codecpar, codecpar);

            // 保存输入流的时间基
            base_time_map.insert(i as c_int, (*in_st).time_base);
        }

        Ok(v_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::context::format::demuxer::{AvioResource, ParamRepairState};
    use rsmpeg::avformat::{AVFormatContextInput, AVIOContextContainer, AVIOContextCustom};
    use rsmpeg::avutil::AVMem;
    use rsmpeg::ffi::{AVSampleFormat_AV_SAMPLE_FMT_FLTP, av_new_packet};

    fn assert_demuxable(init: &Bytes, media: &[u8]) {
        let mut input = Vec::with_capacity(init.len() + media.len());
        input.extend_from_slice(init);
        input.extend_from_slice(media);
        let io = AVIOContextCustom::alloc_context(
            AVMem::new(DEFAULT_IO_BUF_SIZE),
            false,
            input,
            Some(Box::new(|input, output| {
                if input.is_empty() {
                    return rsmpeg::ffi::AVERROR_EOF;
                }
                let len = input.len().min(output.len());
                output[..len].copy_from_slice(&input[..len]);
                input.drain(..len);
                len as i32
            })),
            None,
            None,
        );
        let context = AVFormatContextInput::from_io_context(AVIOContextContainer::Custom(io))
            .expect("init plus HLS parent segment must be parseable as fragmented MP4");
        assert!(context.streams().num() > 0);
    }

    fn assert_complete_mp4_boxes(data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            assert!(data.len() - offset >= 8, "truncated MP4 box header");
            let size32 = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            let (header_size, box_size) = if size32 == 1 {
                assert!(
                    data.len() - offset >= 16,
                    "truncated extended MP4 box header"
                );
                (
                    16,
                    u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap()) as usize,
                )
            } else if size32 == 0 {
                (8, data.len() - offset)
            } else {
                (8, size32)
            };
            assert!(box_size >= header_size, "invalid MP4 box size");
            assert!(box_size <= data.len() - offset, "truncated MP4 box payload");
            offset += box_size;
        }
        assert_eq!(offset, data.len());
    }

    #[test]
    fn audio_only_muxer_emits_parseable_cmaf_parts_and_a_complete_parent() {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            assert!(!fmt_ctx.is_null());
            let stream = avformat_new_stream(fmt_ctx, ptr::null());
            assert!(!stream.is_null());
            (*stream).time_base = AVRational {
                num: 1,
                den: 48_000,
            };
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_AUDIO;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_AAC;
            (*codecpar).sample_rate = 48_000;
            (*codecpar).channels = 1;
            (*codecpar).channel_layout = 4;
            (*codecpar).format = AVSampleFormat_AV_SAMPLE_FMT_FLTP as i32;
            (*codecpar).frame_size = 1024;
            (*codecpar).extradata = av_malloc(2) as *mut u8;
            assert!(!(*codecpar).extradata.is_null());
            *(*codecpar).extradata = 0x11;
            *(*codecpar).extradata.add(1) = 0x88;
            (*codecpar).extradata_size = 2;

            let mut demuxer = DemuxerContext {
                avio: AvioResource {
                    fmt_ctx,
                    io_buf: ptr::null_mut(),
                    avio_ctx: ptr::null_mut(),
                },
                params: Vec::<ParamRepairState>::new(),
            };
            let sender = MuxPacketSender::new(16);
            let mut receiver = sender.subscribe();
            let mut muxer = HlsFmp4Context::init_context(&demuxer, sender).unwrap();
            assert_eq!(&muxer.init_segment[4..8], b"ftyp");

            for index in 0..150 {
                let mut packet = std::mem::zeroed::<AVPacket>();
                assert_eq!(av_new_packet(&mut packet, 4), 0);
                packet.stream_index = 0;
                packet.pts = index * 1024;
                packet.dts = packet.pts;
                packet.duration = 1024;
                ptr::copy_nonoverlapping(b"aac!".as_ptr(), packet.data, 4);
                muxer.write_packet(&packet, 0).unwrap();
                av_packet_unref(&mut packet);
            }
            muxer.flush();
            let init_segment = muxer.init_segment.clone();

            let mut parts = Vec::new();
            while let Ok(packet) = receiver.try_recv() {
                parts.push(packet);
            }
            assert!(parts.len() >= 3);
            assert!(parts.iter().any(|packet| {
                packet
                    .hls
                    .as_ref()
                    .is_some_and(|part| part.segment_complete)
            }));
            for packet in &parts {
                assert_complete_mp4_boxes(&packet.data);
                assert!(packet.data.windows(4).any(|window| window == b"moof"));
                assert!(packet.data.windows(4).any(|window| window == b"mdat"));
                assert!(
                    packet
                        .hls
                        .as_ref()
                        .is_some_and(|part| part.duration_us <= HLS_PART_TARGET_US)
                );
            }
            let first_segment_seq = parts[0].hls.as_ref().unwrap().segment_seq;
            let parent = parts
                .iter()
                .filter(|packet| {
                    packet
                        .hls
                        .as_ref()
                        .is_some_and(|part| part.segment_seq == first_segment_seq)
                })
                .fold(Vec::new(), |mut parent, packet| {
                    parent.extend_from_slice(&packet.data);
                    parent
                });
            assert_demuxable(&init_segment, &parent);

            rsmpeg::ffi::avformat_free_context(demuxer.avio.fmt_ctx);
            demuxer.avio.fmt_ctx = ptr::null_mut();
        }
    }
}
