use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{FmtMuxer, MuxPacket, write_callback};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, info, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVFMT_FLAG_AUTO_BSF, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_FLUSH_PACKETS,
    AVFMT_FLAG_NOBUFFER, AVFMT_NOFILE, AVFormatContext, AVIOContext,
    AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_SUBTITLE,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational, av_free, av_guess_format, av_malloc,
    av_packet_ref, av_packet_rescale_ts, av_packet_unref, av_rescale_q, avcodec_parameters_copy,
    avformat_alloc_context, avformat_new_stream, avformat_write_header, avio_alloc_context,
    avio_context_free,
};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use std::ffi::{CStr, CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use log::error;

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());
pub struct CmafFmp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: broadcast::Sender<Arc<MuxPacket>>,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    in_time_bases: Vec<AVRational>,

    video_stream_index: i32,
    started: bool,
}
impl Drop for CmafFmp4Context {
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
impl FmtMuxer for CmafFmp4Context {
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

            let fmt_ctx = avformat_alloc_context();
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = av_guess_format(MP4.as_ptr(), ptr::null(), ptr::null());
            (*fmt_ctx).max_delay = 0;
            // (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            (*fmt_ctx).flags |= AVFMT_NOFILE as i32;
            (*fmt_ctx).flags |= AVFMT_FLAG_AUTO_BSF as i32;
            if (*fmt_ctx).oformat.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }

            // === CMAF flags ===
            // 创建AVDictionary
            let mut options = ptr::null_mut::<rsmpeg::ffi::AVDictionary>();

            // 设置movflags
            let movflags = CString::new("frag_keyframe+empty_moov+default_base_moof").unwrap();
            rsmpeg::ffi::av_dict_set(
                &mut options,
                CString::new("movflags").unwrap().as_ptr(),
                movflags.as_ptr(),
                0,
            );

            // // // 设置frag_duration为2秒
            // let frag_duration = CString::new("2000000").unwrap();
            // rsmpeg::ffi::av_dict_set(
            //     &mut options,
            //     CString::new("frag_duration").unwrap().as_ptr(),
            //     frag_duration.as_ptr(),
            //     0,
            // );
            // //
            // // // 设置其他CMAFFMP4参数
            // let min_frag_duration = CString::new("1000000").unwrap();
            // rsmpeg::ffi::av_dict_set(
            //     &mut options,
            //     CString::new("min_frag_duration").unwrap().as_ptr(),
            //     min_frag_duration.as_ptr(),
            //     0,
            // );
            // // 设置 GOP 大小相关参数
            // let gop_size = CString::new("25").unwrap(); // 25帧一个GOP
            // rsmpeg::ffi::av_dict_set(
            //     &mut options,
            //     CString::new("g").unwrap().as_ptr(),
            //     gop_size.as_ptr(),
            //     0,
            // );
            // 确保时间戳连续
            // let avoid_negative_ts = CString::new("make_non_negative").unwrap();
            // rsmpeg::ffi::av_dict_set(
            //     &mut options,
            //     CString::new("avoid_negative_ts").unwrap().as_ptr(),
            //     avoid_negative_ts.as_ptr(),
            //     0,
            // );

            let (video_si, in_tbs) = params_trans(demuxer_context, fmt_ctx)?;

            let ret = avformat_write_header(fmt_ctx, &mut options);
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
                fmt_ctx,
                avio_ctx,
                io_buf,
                out_buf_ptr,
                in_time_bases: in_tbs,
                video_stream_index: video_si,
                started: false,
            })
        }
    }
    fn get_header(&self) -> Bytes {
        self.init_segment.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) {
        unsafe {
            let is_video = pkt.stream_index == self.video_stream_index;
            let is_key = is_video && (pkt.flags & AV_PKT_FLAG_KEY as i32 != 0);

            // === 1. fragment 切分点：仅 video keyframe ===
            if is_key {
                self.started = true;
                if self.started {
                    // flush 上一个 fragment
                    self.emit_fragment(timestamp, true);
                } else {
                    self.started = true;
                }
            }

            if !self.started {
                return;
            }
            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);
            // === 2. 时间基 rescale（唯一允许的操作）===
            let in_tb = self.in_time_bases[pkt.stream_index as usize];
            let out_st = *(*self.fmt_ctx).streams.add(pkt.stream_index as usize);
            // (*out_st).time_base = AVRational { num: 1, den: 16000 };
            av_packet_rescale_ts(&mut cloned, in_tb, (*out_st).time_base);
            cloned.pos = -1;

            // === 3. 写入 packet ===
            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);

            if ret < 0 {
                warn!("fMP4 write failed: {}", show_ffmpeg_error_msg(ret));
                return;
            }
            // === 4. 立即收集输出 ===
            if is_key {
                self.emit_fragment(timestamp, is_key);
            }
        }
    }

    fn flush(&mut self) {}
}

impl CmafFmp4Context {
    unsafe fn emit_fragment(&mut self, timestamp: u64, is_key: bool) {
        rsmpeg::ffi::av_write_frame(self.fmt_ctx, ptr::null_mut());
        rsmpeg::ffi::avio_flush((*self.fmt_ctx).pb);
        let out_vec = &mut *self.out_buf_ptr;
        if out_vec.is_empty() {
            return;
        }

        let data = Bytes::from(std::mem::take(out_vec));
        let _ = self.pkt_tx.send(Arc::new(MuxPacket {
            data,
            is_key,
            timestamp,
        }));
    }
}
fn params_trans(
    demuxer_context: &DemuxerContext,
    out_fmt: *mut AVFormatContext,
) -> GlobalResult<(i32, Vec<AVRational>)> {
    unsafe {
        let in_fmt = demuxer_context.avio.fmt_ctx;

        let mut packet_time_bases = Vec::new();
        let mut video_out_index: i32 = -1;
        for i in 0..demuxer_context.params.len() {
            let in_st = *(*in_fmt).streams.offset(i as isize);
            let codecpar = (*in_st).codecpar;

            if !matches!(
                (*codecpar).codec_type,
                AVMediaType_AVMEDIA_TYPE_VIDEO | AVMediaType_AVMEDIA_TYPE_AUDIO
            ) {
                continue;
            }

            let out_st = avformat_new_stream(out_fmt, ptr::null_mut());
            if out_st.is_null() {
                return Err(GlobalError::new_sys_error(
                    "avformat_new_stream failed",
                    |_| {},
                ));
            }

            // copy codecpar
            avcodec_parameters_copy((*out_st).codecpar, codecpar);

            let out_index = (*out_fmt).nb_streams as i32 - 1;

            // === CMAF-mandated time_base ===
            match (*codecpar).codec_type {
                AVMediaType_AVMEDIA_TYPE_VIDEO => {
                    (*out_st).time_base = (*in_st).time_base;
                    // (*out_st).time_base = AVRational { num: 1, den: 90000 };
                    // // 设置帧率
                    // (*out_st).avg_frame_rate = AVRational { num: 25, den: 1 }; // 25fps
                    // (*out_st).r_frame_rate = AVRational { num: 25, den: 1 };
                    // (*out_st).time_base = AVRational { num: 1, den: 90000 };
                    video_out_index = out_index;
                }
                AVMediaType_AVMEDIA_TYPE_AUDIO => {
                    (*out_st).time_base = (*in_st).time_base;
                    // let sr = (*codecpar).sample_rate.max(1);
                    // (*out_st).time_base = AVRational { num: 1, den: sr };
                }
                _ => {}
            }

            // codec_tag (mp4 required)
            (*(*out_st).codecpar).codec_tag = 0;
            // match (*codecpar).codec_id {
            //     AVCodecID_AV_CODEC_ID_H264 => {
            //         (*(*out_st).codecpar).codec_tag = rsmpeg::ffi::MKTAG(b'a', b'v', b'c', b'1');
            //     }
            //     AVCodecID_AV_CODEC_ID_HEVC => {
            //         (*(*out_st).codecpar).codec_tag = rsmpeg::ffi::MKTAG(b'h', b'e', b'v', b'1');
            //     }
            //     AVCodecID_AV_CODEC_ID_AAC => {
            //         (*(*out_st).codecpar).codec_tag = rsmpeg::ffi::MKTAG(b'm', b'p', b'4', b'a');
            //     }
            //     _ => {}
            // }

            // === 保存 packet 原始 time_base（用于 rescale）===
            let in_st = *(*in_fmt).streams.offset(i as isize);
            packet_time_bases.push((*in_st).time_base);

            println!(
                "stream map: in={} -> out={}, codec={:?}, pkt_tb={}/{} out_tb={}/{},codecpar_size={},height-width=({},{}),pixel_format={}",
                i,
                out_index,
                (*codecpar).codec_id,
                (*in_st).time_base.num,
                (*in_st).time_base.den,
                (*out_st).time_base.num,
                (*out_st).time_base.den,
                (*(*out_st).codecpar).extradata_size,
                (*(*out_st).codecpar).height,
                (*(*out_st).codecpar).width,
                (*(*out_st).codecpar).format,
            );
        }

        if video_out_index < 0 {
            return Err(GlobalError::new_sys_error("no video stream found", |msg| error!("{msg}")));
        }

        Ok((video_out_index, packet_time_bases))
    }
}
