use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::fmp4::CmafFmp4Context;
use crate::media::context::format::{FmtMuxer, MuxPacket, fmp4, write_callback};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use axum::body::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::once_cell::sync::Lazy;
use base::tokio::sync::broadcast;
use base::tokio::sync::broadcast::Sender;
use log::{debug, error, info, warn};
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AVFMT_FLAG_AUTO_BSF, AVFMT_FLAG_FLUSH_PACKETS, AVFMT_NOFILE,
    AVFormatContext, AVIOContext, AVPacket, av_dict_set, av_free, av_guess_format,
    av_interleaved_write_frame, av_malloc, av_packet_ref, av_packet_rescale_ts, av_packet_unref,
    av_write_frame, av_write_trailer, avformat_alloc_context, avformat_write_header,
    avio_alloc_context, avio_context_free, avio_flush,
};
use std::collections::HashMap;
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::time::Instant;

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());
pub struct DashCmafMp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: Sender<Arc<MuxPacket>>,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    in_timebase_map: HashMap<c_int, AVRational>,
    v_idx: c_int,
    fragment_started_with_key: bool, // 当前片段是否以关键帧开始
    fragment_start_timestamp: u64,   // 当前片段的第一帧时间戳
    pub epoch: Instant,
    last_dts: i64,
}
impl Drop for DashCmafMp4Context {
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
impl FmtMuxer for DashCmafMp4Context {
    fn init_context(
        demuxer_context: &DemuxerContext,
        pkt_tx: Sender<Arc<MuxPacket>>,
    ) -> GlobalResult<Self>
    where
        Self: Sized,
    {
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
            let movflags = CString::new("frag_keyframe+empty_moov+default_base_moof+dash").unwrap();
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
            let v_idx = fmp4::copy_streams(&mut in_timebase_map, in_fmt_ctx, out_fmt_ctx)?;

            let ret = avformat_write_header(out_fmt_ctx, &mut options);
            // 释放选项字典
            if !options.is_null() {
                rsmpeg::ffi::av_dict_free(&mut options);
            }
            if ret < 0 {
                return Err(GlobalError::new_sys_error(
                    &format!(
                        "Dash MP4 header write failed: {}",
                        show_ffmpeg_error_msg(ret)
                    ),
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
                epoch: Instant::now(),
                last_dts: i64::MIN,
            })
        }
    }

    fn get_header(&self) -> Bytes {
        self.init_segment.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()> {
        unsafe {
            let mut cloned = std::mem::zeroed::<AVPacket>();

            match self.in_timebase_map.get(&pkt.stream_index) {
                None => {
                    warn!(
                        "dash MP4 write failed,stream index error: {}",
                        &pkt.stream_index
                    );
                    return Ok(());
                }
                Some(&in_tb) => {
                    if pkt.dts < self.last_dts || pkt.dts - self.last_dts > 8 * in_tb.den as i64 {
                        return Err(GlobalError::new_biz_error(
                            600,
                            "current dts < last dts or dts cross max limit",
                            |msg| info!("{msg};last: {},current: {}",self.last_dts,pkt.dts),
                        ));
                    }
                    self.last_dts = pkt.dts;
                    // 写入当前帧
                    av_packet_ref(&mut cloned, pkt);
                    if cloned.pts == AV_NOPTS_VALUE {
                        cloned.pts = cloned.dts;
                    }
                    let out_st = *(*self.fmt_ctx).streams.add(pkt.stream_index as usize);
                    av_packet_rescale_ts(&mut cloned, in_tb, (*out_st).time_base);
                    cloned.pos = -1;
                    let ret = av_interleaved_write_frame(self.fmt_ctx, &mut cloned);
                    if ret < 0 {
                        warn!("dash MP4 write failed:{}", show_ffmpeg_error_msg(ret));
                        //尝试修正dts/pts
                        cloned.dts += 1;
                        cloned.pts += 1;
                        if av_interleaved_write_frame(self.fmt_ctx, &mut cloned) < 0 {
                            warn!(
                                "dash MP4 fix dts/pts write failed:{}",
                                show_ffmpeg_error_msg(ret)
                            );
                        } else {
                            info!("dash MP4 fix dts/pts write succeed")
                        }
                        av_packet_unref(&mut cloned);
                        return Ok(());
                    }
                    av_packet_unref(&mut cloned);
                    let is_keyframe =
                        self.v_idx == pkt.stream_index && (pkt.flags & AV_PKT_FLAG_KEY as i32) != 0;
                    if self.flush_fragment(
                        self.fragment_start_timestamp,
                        self.fragment_started_with_key,
                    ) {
                        self.fragment_started_with_key = is_keyframe;
                        self.fragment_start_timestamp = timestamp;
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
impl DashCmafMp4Context {
    fn force_flush_fragment(&mut self, timestamp: u64, is_key: bool) -> bool {
        unsafe {
            // 1. 先写入空帧强制刷新内部缓冲区
            av_write_frame(self.fmt_ctx, ptr::null_mut());

            // 2. 刷新IO缓冲区
            avio_flush((*self.fmt_ctx).pb);
        }
        self.flush_fragment(timestamp, is_key)
    }
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
            let data = Bytes::from(std::mem::take(out_vec));
            let _ = self.pkt_tx.send(Arc::new(MuxPacket {
                data,
                is_key,
                timestamp,
                epoch: self.epoch,
                seq: 0,
            }));
            true
        }
    }
}
