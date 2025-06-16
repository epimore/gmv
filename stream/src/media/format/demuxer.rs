use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::Arc;
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::once_cell::sync::Lazy;
use rsmpeg::ffi::{av_dict_set, av_find_input_format, av_free, av_malloc, avcodec_parameters_alloc, avcodec_parameters_copy, avcodec_parameters_free, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_free_context, avformat_open_input, avio_alloc_context, AVCodecParameters, AVDictionary, AVFormatContext, AVIOContext, AVMediaType_AVMEDIA_TYPE_VIDEO, AVFMT_FLAG_CUSTOM_IO};
use crate::media::{show_ffmpeg_error_msg, rtp, rw};
use crate::media::rw::SdpMemory;

static SDP_FLAGS: Lazy<CString> = Lazy::new(|| CString::new("sdp_flags").unwrap());
static CUSTOM_IO: Lazy<CString> = Lazy::new(|| CString::new("custom_io").unwrap());
static SDP: Lazy<CString> = Lazy::new(|| CString::new("sdp").unwrap());

/// FFmpeg资源自动释放结构
pub struct AvioResource {
    pub fmt_ctx: *mut AVFormatContext,
    pub sdp_io_buf: *mut u8,
    pub rtp_io_buf: *mut u8,
    pub sdp_avio_ctx: *mut AVIOContext,
    pub rtp_avio_ctx: *mut AVIOContext,
    pub original_pb: *mut AVIOContext,
}
unsafe impl Send for AvioResource {}
unsafe impl Sync for AvioResource {}
impl Drop for AvioResource {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                (*self.fmt_ctx).pb = self.original_pb;
                avformat_close_input(&mut self.fmt_ctx);
                avformat_free_context(self.fmt_ctx);
            }
            if !self.sdp_io_buf.is_null() {
                av_free(self.sdp_io_buf as *mut c_void);
            }
            if !self.rtp_io_buf.is_null() {
                av_free(self.rtp_io_buf as *mut c_void);
            }
            if !self.sdp_avio_ctx.is_null() {
                av_free(self.sdp_avio_ctx as *mut c_void);
            }
            if !self.rtp_avio_ctx.is_null() {
                av_free(self.rtp_avio_ctx as *mut c_void);
            }
        }
    }
}

#[derive(Clone)]
pub struct DemuxerContext {
    pub avio: Arc<AvioResource>,
    pub codecpar_list: Vec<*mut AVCodecParameters>,
    pub stream_mapping: Vec<(usize, bool)>,
}
impl Drop for DemuxerContext {
    fn drop(&mut self) {
        unsafe {
            for &par in &self.codecpar_list {
                if !par.is_null() {
                    avcodec_parameters_free(&mut (par as *mut _));
                }
            }
        }
    }
}

impl DemuxerContext {
    pub fn start_demuxer(sdp_map: &(u8, String), mut rtp_buffer: rtp::RtpPacketBuffer) -> GlobalResult<Self> {
        let sdp = build_sdp(sdp_map.0, &sdp_map.1);
        let mut sdp_mem = SdpMemory::new(sdp);
        unsafe {
            //内存中读取sdp信息
            let sdp_io_buf = av_malloc(2048) as *mut u8;
            let sdp_avio_ctx = avio_alloc_context(
                sdp_io_buf,
                2048,
                0,
                &mut sdp_mem as *mut _ as *mut c_void,
                Some(rw::read_sdp_packet),
                None,
                None,
            );
            let fmt_ctx = avformat_alloc_context();
            (*fmt_ctx).pb = sdp_avio_ctx;
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();
            av_dict_set(
                &mut dict_opts,
                SDP_FLAGS.as_ptr(),
                CUSTOM_IO.as_ptr(),
                0,
            );
            let input_fmt = av_find_input_format(SDP.as_ptr());
            let ret = avformat_open_input(
                &mut (fmt_ctx as *mut _),
                ptr::null(),
                input_fmt,
                &mut dict_opts,
            );
            rsmpeg::ffi::av_dict_free(&mut dict_opts);
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open sdp input:ret= {ret}, msg={msg}")));
            }
            //创建 RTP AVIOContext
            let rtp_buf_ptr = &mut rtp_buffer as *mut _ as *mut c_void;
            let rtp_io_buf = av_malloc(4096) as *mut u8;
            let rtp_avio_ctx = avio_alloc_context(
                rtp_io_buf,
                4096,
                1,
                rtp_buf_ptr,
                Some(rw::read_rtp_packet),
                Some(rw::write_rtcp_packet),
                None,
            );

            //保存原始 pb 并替换为 RTP 数据流
            let original_pb = (*fmt_ctx).pb;
            (*fmt_ctx).pb = rtp_avio_ctx;
            if avformat_find_stream_info(fmt_ctx, ptr::null_mut()) < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Could not find stream info:ret= {ret}, msg={msg}")));
            }

            let mut codecpar_list = Vec::with_capacity((*fmt_ctx).nb_streams as usize);
            let mut stream_mapping = vec![];

            for i in 0..(*fmt_ctx).nb_streams {
                let in_stream = *(*fmt_ctx).streams.add(i as usize);
                let codecpar = avcodec_parameters_alloc();
                if codecpar.is_null() {
                    return Err(GlobalError::new_biz_error(1100, "Failed to alloc AVCodecParameters", |msg| error!("msg={msg}")));
                }
                avcodec_parameters_copy(codecpar, (*in_stream).codecpar);
                let mut is_av = false;
                if (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
                    is_av = true
                }
                codecpar_list.push(codecpar);
                stream_mapping.push((i as usize, is_av));
            }

            let ctx = DemuxerContext {
                avio: Arc::new(AvioResource {
                    fmt_ctx,
                    sdp_io_buf,
                    rtp_io_buf,
                    sdp_avio_ctx,
                    rtp_avio_ctx,
                    original_pb,
                }),
                codecpar_list,
                stream_mapping,
            };
            Ok(ctx)
        }
    }
}

fn build_sdp(rtp_map_key: u8, rtp_map_val: &String) -> String {
    let mut sdp = String::with_capacity(300);
    sdp.push_str("v=0\r\n");
    sdp.push_str("o=- 0 0 IN IP4 127.0.0.1\r\n");
    sdp.push_str("s=No Name\r\n");
    sdp.push_str("c=IN IP4 127.0.0.1\r\n");
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("m=video 0 RTP/AVP {}\r\n", rtp_map_key));
    sdp.push_str(&format!("a=rtpmap:{} {}\r\n", rtp_map_key, rtp_map_val));
    sdp
}