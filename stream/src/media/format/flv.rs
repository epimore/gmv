use std::ffi::{c_int, c_void, CString};
use std::ptr;
use crate::media::format::demuxer;
use crate::media::format::muxer::MuxerSink;
use common::bytes::Bytes;
use common::once_cell::sync::Lazy;
use common::tokio::sync::broadcast;
use rsmpeg::ffi::{av_guess_format, av_malloc, avcodec_parameters_copy, avformat_alloc_context, avformat_new_stream, avio_alloc_context, AVFormatContext, AVIOContext, AVPacket};

static FLV: Lazy<CString> = Lazy::new(|| CString::new("flv").unwrap());
pub struct FlvMuxer {
    pub flv_header: Bytes,
    //is_idr packet
    pub flv_body_tx: broadcast::Sender<(bool, Bytes)>,
    pub fmt_ctx: *mut AVFormatContext,
    pub avio_ctx: *mut AVIOContext,
    pub io_buf: *mut u8,
}
impl Drop for FlvMuxer {
    fn drop(&mut self) {
        todo!()
    }
}
impl FlvMuxer {
    pub fn new(flv_body_tx: broadcast::Sender<Bytes>, demuxer_context: &demuxer::DemuxerContext) -> Self {
        unsafe {
            let io_buf_size = 4096;
            let io_buf = av_malloc(io_buf_size) as *mut u8;

            let avio_ctx = avio_alloc_context(
                io_buf,
                io_buf_size as c_int,
                1,
                output_ptr as *mut c_void,
                None,
                Some(Self::write_callback),
                None,
            );
            let fmt_ctx = avformat_alloc_context();
            let flv_fmt = av_guess_format(FLV.as_ptr(), ptr::null(), ptr::null());
            (*fmt_ctx).pb = avio_ctx;
            (*fmt_ctx).oformat = flv_fmt;

            for &codecpar in demuxer_context.codecpar_list {
                let stream = avformat_new_stream(fmt_ctx, ptr::null_mut());
                avcodec_parameters_copy((*stream).codecpar, codecpar);
                (*stream).codecpar.codec_tag = 0;
            }
            self.avio_ctx.write_packet = Some(Self::write_callback);
            //todo 
        }
    }

    pub fn get_header(&self) -> Bytes {
        self.flv_header.clone()
    }

    unsafe extern "C" fn write_callback(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
        todo!()
    }
}

impl MuxerSink for FlvMuxer {
    fn write_packet(&mut self, pkt: &AVPacket) {


        todo!()
    }
}