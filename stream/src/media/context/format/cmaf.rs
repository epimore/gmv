use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{FmtMuxer, MuxPacket, write_callback};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, warn};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use rsmpeg::ffi::{
    AV_PKT_FLAG_KEY, AVFMT_FLAG_FLUSH_PACKETS, AVFormatContext, AVIOContext,
    AVMediaType_AVMEDIA_TYPE_AUDIO, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational, av_free,
    av_guess_format, av_malloc, av_packet_ref, av_packet_rescale_ts, av_packet_unref, av_rescale_q,
    avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avformat_write_header,
    avio_alloc_context, avio_context_free,
};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use crate::media::context::utils::extradata::rebuild_codecpar_extradata_with_ffmpeg;

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());

pub struct CmafFmp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: broadcast::Sender<Arc<MuxPacket>>,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    in_time_bases: Vec<AVRational>,
    out_time_bases: Vec<AVRational>,

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
            (*fmt_ctx).flags |= AVFMT_FLAG_FLUSH_PACKETS as i32;
            if (*fmt_ctx).oformat.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to alloc format context",
                    |msg| warn!("{msg}"),
                ));
            }

            // === CMAF flags ===
            let flags = CString::new("frag_keyframe+empty_moov+default_base_moof+cmaf").unwrap();

            rsmpeg::ffi::av_dict_set(
                &mut (*fmt_ctx).metadata,
                CString::new("movflags").unwrap().as_ptr(),
                flags.as_ptr(),
                0,
            );
            rsmpeg::ffi::av_dict_set(
                &mut (*fmt_ctx).metadata,
                CString::new("frag_duration").unwrap().as_ptr(),
                CString::new("200000").unwrap().as_ptr(),
                0,
            );
            let (video_si, in_tbs, out_tbs) = params_trans(demuxer_context, fmt_ctx)?;

            let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
            if ret < 0 {
                return Err(GlobalError::new_sys_error(
                    &format!("FMP4 header write failed: {}", show_ffmpeg_error_msg(ret)),
                    |msg| warn!("{msg}"),
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
                out_time_bases: out_tbs,
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
            let mut cloned = std::mem::zeroed::<AVPacket>();
            av_packet_ref(&mut cloned, pkt);

            let si = pkt.stream_index as usize;

            // === 关键帧起播 ===
            if !self.started {
                if pkt.stream_index == self.video_stream_index
                    && (pkt.flags & AV_PKT_FLAG_KEY as i32 != 0)
                {
                    self.started = true;
                } else {
                    av_packet_unref(&mut cloned);
                    return;
                }
            }

            av_packet_rescale_ts(&mut cloned, self.in_time_bases[si], self.out_time_bases[si]);

            let ret = rsmpeg::ffi::av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
            av_packet_unref(&mut cloned);

            if ret < 0 {
                warn!("fMP4 write failed: {}", show_ffmpeg_error_msg(ret));
                return;
            }

            let out_vec = &mut *self.out_buf_ptr;
            if out_vec.is_empty() {
                return;
            }

            let data = Bytes::from(std::mem::take(out_vec));

            let is_key = pkt.stream_index == self.video_stream_index
                && (pkt.flags & AV_PKT_FLAG_KEY as i32 != 0);

            let _ = self.pkt_tx.send(Arc::new(MuxPacket {
                data,
                is_key,
                timestamp,
            }));
        }
    }

    fn flush(&mut self) {}
}
fn params_trans(
    demuxer_context: &DemuxerContext,
    fmt_ctx: *mut AVFormatContext,
) -> GlobalResult<(i32, Vec<AVRational>, Vec<AVRational>)> {
    unsafe {
        let in_fmt = demuxer_context.avio.fmt_ctx;
        let nb_streams = demuxer_context.params.len();
        let mut in_tbs = Vec::with_capacity(nb_streams);
        let mut out_tbs = Vec::with_capacity(nb_streams);
        let mut video_si: i32 = -1;

        for i in 0..nb_streams {
            // 输入 AVStream
            let in_st = *(*in_fmt).streams.offset(i as isize);

            // 输出 AVStream
            let out_st = avformat_new_stream(fmt_ctx, ptr::null_mut());
            if out_st.is_null() {
                return Err(GlobalError::new_sys_error(
                    "Failed to create new AVStream",
                    |_| {},
                ));
            }
            let out_par = (*out_st).codecpar;
            // 复制 codecpar
            let param_stream = demuxer_context.params.get(i).unwrap();
            avcodec_parameters_copy(out_par, param_stream.codecpar);

            if matches!(
                (*out_par).codec_id,
                AVCodecID_AV_CODEC_ID_H264 | AVCodecID_AV_CODEC_ID_HEVC | AVCodecID_AV_CODEC_ID_AAC
            ) {
                rebuild_codecpar_extradata_with_ffmpeg(param_stream.codecpar, out_par).map_err(|e| {
                    GlobalError::new_sys_error(
                        &format!("rebuild extradata failed: {}", show_ffmpeg_error_msg(e)),
                        |_| {},
                    )
                })?;
            }

            // 时间基
            (*out_st).time_base = (*in_st).time_base;

            // 视频流索引
            if (*(*out_st).codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
                video_si = i as i32;
            }

            // codec_tag 清零
            (*(*out_st).codecpar).codec_tag = 0;

            // 保存 input/output time_base
            in_tbs.push((*in_st).time_base);
            out_tbs.push((*out_st).time_base);
        }

        // 返回 video stream index 以及 tbs
        Ok((video_si, in_tbs, out_tbs))
    }
}
