use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{FmtMuxer, MuxPacket, write_callback};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, info, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use log::error;
use rsmpeg::ffi::{AV_PKT_FLAG_KEY, AVFMT_FLAG_AUTO_BSF, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_FLUSH_PACKETS, AVFMT_FLAG_NOBUFFER, AVFMT_NOFILE, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational, AVStream, av_dict_set, av_free, av_guess_format, av_interleaved_write_frame, av_malloc, av_packet_ref, av_packet_rescale_ts, av_packet_unref, av_rescale_q, av_write_frame, av_write_trailer, avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context, avio_context_free, avio_flush, AV_NOPTS_VALUE};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use rtp_types::prelude::PayloadLength;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_int, c_uint, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());
const MAX_DURATION: Duration = Duration::from_millis(500);
pub struct HlsFmp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: broadcast::Sender<Arc<MuxPacket>>,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    in_timebase_map: HashMap<c_int, AVRational>,
    v_idx: c_int,
    fragment_started_with_key: bool, // 当前片段是否以关键帧开始
    fragment_start_timestamp: u64,   // 当前片段的第一帧时间戳
    last_dts: i64,
    pub epoch: Instant, //当由于seek导致dts回退时，重新初始化mux cxt
    pub seq: usize, //hls 片段序号
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
        pkt_tx: broadcast::Sender<Arc<MuxPacket>>,
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

            // 设置movflags frag_keyframe frag_custom frag_every_frame cmaf dash
            let movflags = CString::new("frag_keyframe+empty_moov+default_base_moof").unwrap();
            rsmpeg::ffi::av_dict_set(
                &mut options,
                CString::new("movflags").unwrap().as_ptr(),
                movflags.as_ptr(),
                0,
            );
            let frag_duration = CString::new("2000000").unwrap(); // 2s
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
                fragment_started_with_key: true,
                fragment_start_timestamp: 0,
                // fragment_frame_count: 0,
                last_dts: i64::MIN,
                epoch: Instant::now(),
                seq: 0,
            })
        }
    }
    fn get_header(&self) -> Bytes {
        self.init_segment.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()> {
        unsafe {
            if pkt.dts < self.last_dts {
                return Err(GlobalError::new_biz_error(
                    600,
                    "current dts < last dts",
                    |msg| info!("{msg};last: {},current: {}",self.last_dts,pkt.dts),
                ));
            }
            self.last_dts = pkt.dts;
            let mut cloned = std::mem::zeroed::<AVPacket>();

            match self.in_timebase_map.get(&pkt.stream_index) {
                None => {
                    warn!(
                        "fMP4 write failed,stream index error: {}",
                        &pkt.stream_index
                    );
                    return Ok(());
                }
                Some(&in_tb) => {
                    // 写入当前帧
                    av_packet_ref(&mut cloned, pkt);
                    if cloned.pts == AV_NOPTS_VALUE {
                        cloned.pts = cloned.dts;
                    }
                    let out_st = *(*self.fmt_ctx).streams.add(pkt.stream_index as usize);
                    av_packet_rescale_ts(&mut cloned, in_tb, (*out_st).time_base);

                    // if self.fragment_frame_count == 3 {
                    //     cloned.pts += 1;
                    //     cloned.dts += 1;
                    // }

                    cloned.pos = -1;
                    let ret = av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
                    if ret < 0 {
                        warn!("FMP4 write failed:{}", show_ffmpeg_error_msg(ret));
                        //尝试修正dts/pts
                        cloned.dts += 1;
                        cloned.pts += 1;
                        if av_interleaved_write_frame(self.fmt_ctx, &mut cloned) < 0 {
                            warn!(
                                "FMP4 fix dts/pts write failed:{}",
                                show_ffmpeg_error_msg(ret)
                            );
                        } else {
                            info!("FMP4 fix dts/pts write succeed")
                        }
                        av_packet_unref(&mut cloned);
                        return Ok(());
                    }
                    // self.fragment_frame_count += 1;
                    av_packet_unref(&mut cloned);
                    let is_keyframe =
                        self.v_idx == pkt.stream_index && (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0;
                    if self.flush_fragment(
                        self.fragment_start_timestamp,
                        self.fragment_started_with_key,
                    ) {
                        self.fragment_started_with_key = is_keyframe;
                        self.fragment_start_timestamp = timestamp;
                        // self.fragment_frame_count = 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn flush(&mut self) {
        unsafe {
            // 1. 写入所有缓冲帧
            av_write_frame(self.fmt_ctx, ptr::null_mut());

            // 2. 写入尾部信息
            av_write_trailer(self.fmt_ctx);

            // 3. 刷新并发送最后一个片段
            avio_flush((*self.fmt_ctx).pb);
            self.flush_fragment(
                self.fragment_start_timestamp,
                self.fragment_started_with_key,
            );
        }
    }
}

impl HlsFmp4Context {
    fn flush_fragment(&mut self, timestamp: u64, is_key: bool) -> bool {
        unsafe {
            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                return false;
            }
            debug!(
                "Flushing fragment: {} bytes, starts_with_key={}, timestamp={}",
                out_vec.len(),
                is_key,
                timestamp
            );
            self.seq += 1;
            let data = Bytes::from(std::mem::take(out_vec));
            let _ = self.pkt_tx.send(Arc::new(MuxPacket {
                data,
                is_key,
                timestamp,
                epoch: self.epoch,
                seq: self.seq,
            }));
            true
        }
    }
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
