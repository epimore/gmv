use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::{
    FmtMuxer, MuxPacket, MuxPacketSender, PlannedStreamMap, can_start_fragmented_output,
    copy_output_plan, write_callback,
};
use crate::media::{DEFAULT_IO_BUF_SIZE, show_ffmpeg_error_msg};
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, info, warn};
use base::once_cell::sync::Lazy;
use log::error;
use rsmpeg::avcodec::AVPacket as OwnedAvPacket;
use rsmpeg::ffi::{
    AV_NOPTS_VALUE, AV_PKT_FLAG_KEY, AVFMT_FLAG_AUTO_BSF, AVFMT_FLAG_CUSTOM_IO,
    AVFMT_FLAG_FLUSH_PACKETS, AVFMT_FLAG_NOBUFFER, AVFMT_NOFILE, AVFormatContext, AVIOContext,
    AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVPacket, AVRational, AVStream, av_dict_set, av_free,
    av_guess_format, av_interleaved_write_frame, av_malloc, av_packet_ref, av_packet_rescale_ts,
    av_rescale_q, av_write_frame, av_write_trailer, avformat_alloc_context, avformat_write_header,
    avio_alloc_context, avio_context_free, avio_flush,
};
use rsmpeg::ffi::{
    AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
};
use rtp_types::prelude::PayloadLength;
use std::ffi::{CStr, CString, c_int, c_uint, c_void};
use std::ptr;
use std::slice;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

static MP4: Lazy<CString> = Lazy::new(|| CString::new("mp4").unwrap());
const MAX_DURATION: Duration = Duration::from_millis(500);
pub struct CmafFmp4Context {
    pub init_segment: Bytes, // CMAF init.mp4
    pub pkt_tx: MuxPacketSender,

    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
    out_buf_ptr: *mut Vec<u8>,

    stream_map: PlannedStreamMap,
    v_idx: c_int,
    started: bool,
    fragment_started_with_key: bool, // 当前片段是否以关键帧开始
    fragment_start_timestamp: u64,   // 当前片段的第一帧时间戳
    pub epoch: Instant,              //当由于seek导致dts回退时，重新初始化mux cxt
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

            // 设置movflags frag_keyframe frag_custom frag_every_frame cmaf dash
            let movflags = CString::new("frag_keyframe+empty_moov+default_base_moof").unwrap();
            rsmpeg::ffi::av_dict_set(
                &mut options,
                CString::new("movflags").unwrap().as_ptr(),
                movflags.as_ptr(),
                0,
            );
            let frag_duration = CString::new("500000").unwrap(); // 500ms
            av_dict_set(
                &mut options,
                CString::new("frag_duration").unwrap().as_ptr(),
                frag_duration.as_ptr(),
                0,
            );
            let (stream_map, v_idx) = copy_output_plan(demuxer_context, out_fmt_ctx)?;

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
                stream_map,
                v_idx,
                started: false,
                fragment_started_with_key: false,
                fragment_start_timestamp: 0,
                epoch: Instant::now(),
            })
        }
    }
    fn get_header(&self) -> Bytes {
        self.init_segment.clone()
    }

    fn write_packet(&mut self, pkt: &AVPacket, timestamp: u64) -> GlobalResult<()> {
        unsafe {
            match self.stream_map.get(&pkt.stream_index) {
                None => {
                    warn!(
                        "fMP4 write failed,stream index error: {}",
                        &pkt.stream_index
                    );
                    return Ok(());
                }
                Some(planned) => {
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
                    if !self.started {
                        self.started = true;
                        self.fragment_started_with_key = self.v_idx < 0 || is_keyframe;
                        self.fragment_start_timestamp = timestamp;
                    }
                    let out_st = *(*self.fmt_ctx).streams.add(planned.output_index as usize);
                    let strip_aac_adts = {
                        let codecpar = (*out_st).codecpar;
                        (*codecpar).codec_id == AVCodecID_AV_CODEC_ID_AAC
                            && !(*codecpar).extradata.is_null()
                            && (*codecpar).extradata_size > 0
                    };
                    let mut cloned = clone_packet_for_mp4(pkt, strip_aac_adts)?;
                    (*cloned.as_mut_ptr()).stream_index = planned.output_index;

                    // 写入当前帧
                    av_packet_rescale_ts(
                        cloned.as_mut_ptr(),
                        planned.input_time_base,
                        (*out_st).time_base,
                    );

                    (*cloned.as_mut_ptr()).pos = -1;
                    let ret = av_interleaved_write_frame(self.fmt_ctx, cloned.as_mut_ptr());
                    if ret < 0 {
                        return Err(GlobalError::new_sys_error(
                            &format!("FMP4 write failed: {}", show_ffmpeg_error_msg(ret)),
                            |msg| error!("{msg}"),
                        ));
                    }
                    // self.fragment_frame_count += 1;
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

impl CmafFmp4Context {
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
                hls: None,
            }));
            true
        }
    }
}
pub(crate) fn clone_packet_for_mp4(
    pkt: &AVPacket,
    strip_aac_adts: bool,
) -> GlobalResult<OwnedAvPacket> {
    let mut cloned = OwnedAvPacket::new();
    let ret = unsafe { av_packet_ref(cloned.as_mut_ptr(), pkt) };
    if ret < 0 {
        return Err(GlobalError::new_sys_error(
            &format!("FMP4 packet clone failed: {}", show_ffmpeg_error_msg(ret)),
            |msg| warn!("{msg}"),
        ));
    }

    if !strip_aac_adts {
        return Ok(cloned);
    }

    let data = unsafe {
        let cloned_ptr = cloned.as_mut_ptr();
        if (*cloned_ptr).data.is_null() || (*cloned_ptr).size <= 0 {
            return Ok(cloned);
        }
        slice::from_raw_parts((*cloned_ptr).data, (*cloned_ptr).size as usize)
    };
    let Some(header_len) = adts_payload_offset(data).map_err(|reason| {
        GlobalError::new_sys_error(
            &format!("FMP4 AAC ADTS normalization failed: {reason}"),
            |msg| warn!("{msg}"),
        )
    })?
    else {
        return Ok(cloned);
    };

    unsafe {
        let cloned_ptr = cloned.as_mut_ptr();
        (*cloned_ptr).data = (*cloned_ptr).data.add(header_len);
        (*cloned_ptr).size -= header_len as c_int;
    }
    Ok(cloned)
}

fn adts_payload_offset(data: &[u8]) -> Result<Option<usize>, &'static str> {
    if data.len() < 2 || data[0] != 0xff || data[1] & 0xf6 != 0xf0 {
        return Ok(None);
    }
    if data.len() < 7 {
        return Err("truncated ADTS header");
    }

    let header_len = if data[1] & 0x01 == 0 { 9 } else { 7 };
    if data.len() < header_len {
        return Err("truncated ADTS CRC header");
    }
    if data[6] & 0x03 != 0 {
        return Err("ADTS frame with multiple raw data blocks is unsupported");
    }

    let frame_len = ((usize::from(data[3] & 0x03)) << 11)
        | (usize::from(data[4]) << 3)
        | usize::from(data[5] >> 5);
    if frame_len <= header_len {
        return Err("ADTS frame has no AAC payload");
    }
    if frame_len != data.len() {
        return Err("AAC packet must contain exactly one complete ADTS frame");
    }

    Ok(Some(header_len))
}

#[cfg(test)]
mod tests {
    use super::{CmafFmp4Context, adts_payload_offset, clone_packet_for_mp4};
    use crate::media::context::format::demuxer::{AvioResource, DemuxerContext};
    use crate::media::context::format::{FmtMuxer, MuxPacketSender};
    use crate::media::rtp::RtpReadControl;
    use base::tokio_util::sync::CancellationToken;
    use rsmpeg::avcodec::AVPacket as OwnedAvPacket;
    use rsmpeg::ffi::{av_new_packet, avformat_alloc_context};
    use std::slice;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn adts_frame(payload: &[u8], has_crc: bool, raw_data_blocks: u8) -> Vec<u8> {
        let header_len = if has_crc { 9 } else { 7 };
        let frame_len = header_len + payload.len();
        let mut frame = vec![0_u8; header_len];
        frame[0] = 0xff;
        frame[1] = if has_crc { 0xf0 } else { 0xf1 };
        frame[2] = 0x6c;
        frame[3] = 0x40 | ((frame_len >> 11) & 0x03) as u8;
        frame[4] = (frame_len >> 3) as u8;
        frame[5] = ((frame_len & 0x07) as u8) << 5 | 0x1f;
        frame[6] = 0xfc | (raw_data_blocks & 0x03);
        if has_crc {
            frame[7] = 0x12;
            frame[8] = 0x34;
        }
        frame.extend_from_slice(payload);
        frame
    }

    fn packet_with_data(data: &[u8]) -> OwnedAvPacket {
        let mut packet = OwnedAvPacket::new();
        let ret = unsafe { av_new_packet(packet.as_mut_ptr(), data.len() as i32) };
        assert_eq!(ret, 0);
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), (*packet.as_mut_ptr()).data, data.len());
        }
        packet
    }

    fn packet_data(packet: &mut OwnedAvPacket) -> &[u8] {
        unsafe {
            let packet = packet.as_mut_ptr();
            slice::from_raw_parts((*packet).data, (*packet).size as usize)
        }
    }

    #[test]
    fn silent_aac_plan_writes_valid_fmp4_init_without_source_audio_parameters() {
        unsafe {
            let fmt_ctx = avformat_alloc_context();
            assert!(!fmt_ctx.is_null());
            let mut demuxer = DemuxerContext {
                avio: AvioResource {
                    fmt_ctx,
                    io_buf: std::ptr::null_mut(),
                    avio_ctx: std::ptr::null_mut(),
                },
                params: Vec::new(),
                read_control: Arc::new(RtpReadControl::new(
                    CancellationToken::new(),
                    Instant::now() + Duration::from_secs(60),
                )),
                output_plan: Default::default(),
            };
            demuxer.output_plan.add_silent_audio();

            let muxer = CmafFmp4Context::init_context(&demuxer, MuxPacketSender::new(4)).unwrap();

            assert_eq!(&muxer.init_segment[4..8], b"ftyp");
        }
    }

    #[test]
    fn strips_adts_without_crc_from_cloned_mp4_packet() {
        let payload = [0x21, 0x10, 0x56, 0xe5];
        let source = packet_with_data(&adts_frame(&payload, false, 0));

        let mut cloned = clone_packet_for_mp4(&source, true).unwrap();

        assert_eq!(packet_data(&mut cloned), payload);
    }

    #[test]
    fn strips_adts_with_crc_from_cloned_mp4_packet() {
        let payload = [0xde, 0xad, 0xbe, 0xef];
        let source = packet_with_data(&adts_frame(&payload, true, 0));

        let mut cloned = clone_packet_for_mp4(&source, true).unwrap();

        assert_eq!(packet_data(&mut cloned), payload);
    }

    #[test]
    fn leaves_raw_aac_packet_unchanged() {
        let payload = [0x21, 0x10, 0x56, 0xe5];
        let source = packet_with_data(&payload);

        let mut cloned = clone_packet_for_mp4(&source, true).unwrap();

        assert_eq!(packet_data(&mut cloned), payload);
    }

    #[test]
    fn leaves_adts_packet_unchanged_without_asc_codec_parameters() {
        let packet = adts_frame(&[0x21, 0x10, 0x56, 0xe5], false, 0);
        let source = packet_with_data(&packet);

        let mut cloned = clone_packet_for_mp4(&source, false).unwrap();

        assert_eq!(packet_data(&mut cloned), packet);
    }

    #[test]
    fn rejects_concatenated_adts_frames() {
        let mut packet = adts_frame(&[1, 2, 3], false, 0);
        packet.extend_from_slice(&adts_frame(&[4, 5, 6], false, 0));

        assert_eq!(
            adts_payload_offset(&packet),
            Err("AAC packet must contain exactly one complete ADTS frame")
        );
    }

    #[test]
    fn rejects_multiple_raw_data_blocks_in_one_adts_frame() {
        let packet = adts_frame(&[1, 2, 3], false, 1);

        assert_eq!(
            adts_payload_offset(&packet),
            Err("ADTS frame with multiple raw data blocks is unsupported")
        );
    }
}
