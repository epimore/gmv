use crate::media::rw::SdpMemory;
use crate::media::{rtp, rw, show_ffmpeg_error_msg};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info};
use base::once_cell::sync::Lazy;
use rsmpeg::ffi::{
    av_dict_set,
    av_find_input_format,
    av_free,
    av_malloc,
    avcodec_parameters_alloc,
    avcodec_parameters_copy,
    avcodec_parameters_free,
    avformat_alloc_context,
    avformat_close_input,
    avformat_find_stream_info,
    avformat_free_context,
    avformat_open_input,
    avio_alloc_context,
    AVCodecParameters,
    AVDictionary,
    AVFormatContext,
    AVIOContext,
    AVMediaType_AVMEDIA_TYPE_VIDEO,
    AVFMT_FLAG_CUSTOM_IO,
};
use shared::info::media_info_ext::MediaExt;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;

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
    // 持有 sdp 内存的原始指针（Box<SdpMemory> 转为裸指针）
    pub sdp_mem_ptr: *mut SdpMemory,
}
unsafe impl Send for AvioResource {}

impl Drop for AvioResource {
    fn drop(&mut self) {
        unsafe {
            if !self.fmt_ctx.is_null() {
                // 恢复原始 pb 再关闭
                (*self.fmt_ctx).pb = self.original_pb;
                avformat_close_input(&mut self.fmt_ctx);
                // avformat_free_context(self.fmt_ctx);
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

            // 回收之前用 Box::into_raw 转出的 sdp memory
            if !self.sdp_mem_ptr.is_null() {
                // 转回 Box 自动 drop
                let _ = Box::from_raw(self.sdp_mem_ptr);
                self.sdp_mem_ptr = ptr::null_mut();
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
    pub fn start_demuxer(ssrc: u32, media_ext: &MediaExt, mut rtp_buffer: rtp::RtpPacketBuffer) -> GlobalResult<Self> {
        let sdp = build_sdp(ssrc, media_ext.tp_code, &media_ext.tp_val);
        info!("sdp: {}", sdp);
        unsafe {
            // 把 SdpMemory 放到堆上并持有其裸指针，保证生命周期
            let sdp_box = Box::new(SdpMemory::new(sdp));
            let sdp_ptr = Box::into_raw(sdp_box);

            // 内存中读取sdp信息
            let sdp_io_buf = av_malloc(2048) as *mut u8;
            let sdp_avio_ctx = avio_alloc_context(
                sdp_io_buf,
                2048,
                0,
                sdp_ptr as *mut c_void,
                Some(rw::read_sdp_packet),
                None,
                None,
            );

            if sdp_avio_ctx.is_null() || (*sdp_avio_ctx).error != 0 {
                error!("Failed to alloc sdp avio context");
            }

            let fmt_ctx = avformat_alloc_context();
            if fmt_ctx.is_null() {
                // 回收 sdp_ptr
                let _ = Box::from_raw(sdp_ptr);
                return Err(GlobalError::new_sys_error("Failed to alloc format context", |msg| error!("{msg}")));
            }

            (*fmt_ctx).pb = sdp_avio_ctx;
            (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO as c_int;
            let mut dict_opts: *mut AVDictionary = ptr::null_mut();

            // 添加处理非单调时间戳的标志
            let fflags_key = CString::new("fflags").unwrap();
            // let fflags_val = CString::new("nobuffer+discardcorrupt+genpts+igndts").unwrap();
            // 修改fflags配置为：
            let fflags_val = CString::new("nobuffer+discardcorrupt+genpts+igndts+sortdts").unwrap();

            av_dict_set(&mut dict_opts, fflags_key.as_ptr(), fflags_val.as_ptr(), 0);

            // 设置最大延迟（微秒）用于数据包重排序
            // let max_delay_key = CString::new("max_delay").unwrap();
            // let max_delay_val = CString::new("500000").unwrap(); // 500ms
            // av_dict_set(&mut dict_opts, max_delay_key.as_ptr(), max_delay_val.as_ptr(), 0);

            // 启用非严格模式处理格式不规范的流
            let strict_std_compliance_key = CString::new("strict").unwrap();
            let strict_std_compliance_val = CString::new("experimental").unwrap();
            av_dict_set(&mut dict_opts, strict_std_compliance_key.as_ptr(), strict_std_compliance_val.as_ptr(), 0);

            // analyzeduration = 5 秒（单位微秒）
            let analyzeduration_key = CString::new("analyzeduration").unwrap();
            let analyzeduration_val = CString::new("8000000").unwrap(); // 5,000,000 us
            av_dict_set(&mut dict_opts, analyzeduration_key.as_ptr(), analyzeduration_val.as_ptr(), 0);
            // 
            // // probesize = 5MB
            // let probesize_key = CString::new("probesize").unwrap();
            // let probesize_val = CString::new("8000000").unwrap(); // 字节
            // av_dict_set(&mut dict_opts, probesize_key.as_ptr(), probesize_val.as_ptr(), 0);
            // let format = CString::new("format").unwrap();
            // let ps = CString::new("mpegps").unwrap();
            // av_dict_set(&mut dict_opts, format.as_ptr(), ps.as_ptr(), 0);
            let ret = av_dict_set(&mut dict_opts, SDP_FLAGS.as_ptr(), CUSTOM_IO.as_ptr(), 0);
            if ret < 0 {
                // 回收 sdp_ptr
                let _ = Box::from_raw(sdp_ptr);
                return Err(GlobalError::new_sys_error(&format!("av_dict_set failed: {}", ret), |msg| error!("{msg}")));
            }
            println!("111111111111111111");
            let input_fmt = av_find_input_format(SDP.as_ptr());
            // let input_fmt = ptr::null_mut();
            let ret = avformat_open_input(
                &mut (fmt_ctx as *mut _),
                ptr::null(),
                input_fmt,
                &mut dict_opts,
            );
            
            if ret < 0 {
                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                // 回收 sdp_ptr
                let _ = Box::from_raw(sdp_ptr);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Failed to open sdp input:ret= {ret}, msg={msg}")));
            }
            println!("222222222222222");
            if input_fmt.is_null() {
                error!("Failed to find input format:FFmpeg未编译PS格式支持");
            }

            // 创建 RTP AVIOContext，注意缓冲区大小保持一致
            let rtp_buf_ptr = &mut rtp_buffer as *mut _ as *mut c_void;
            let rtp_io_buf = av_malloc(8192) as *mut u8;
            let rtp_avio_ctx = avio_alloc_context(
                rtp_io_buf,
                8192,
                1,
                rtp_buf_ptr,
                Some(rw::read_rtp_packet),
                Some(rw::write_rtcp_packet),
                None,
            );
            println!("333333333333333");
            // 保存原始 pb 并替换为 RTP 数据流
            let original_pb = (*fmt_ctx).pb;
            (*fmt_ctx).pb = rtp_avio_ctx;
            let ret = avformat_find_stream_info(fmt_ctx,&mut dict_opts);
            rsmpeg::ffi::av_dict_free(&mut dict_opts);
            println!("44444444444");
            if ret < 0 {
                // 记录详细的流信息用于调试
                for i in 0..(*fmt_ctx).nb_streams {
                    let stream = *(*fmt_ctx).streams.offset(i as isize);
                    error!("流 {}: 类型 = {}, 编解码器ID = {}", 
                            i, 
                            (*stream).codecpar.as_ref().map_or(0, |p| p.codec_type),
                            (*stream).codecpar.as_ref().map_or(0, |p| p.codec_id));
                }

                let ffmpeg_error = show_ffmpeg_error_msg(ret);
                // 清理：恢复 pb，释放 sdp_ptr
                (*fmt_ctx).pb = original_pb;
                let _ = Box::from_raw(sdp_ptr);
                return Err(GlobalError::new_biz_error(1100, &*ffmpeg_error, |msg| error!("Could not find stream info:ret= {ret}, msg={msg}")));
            }
            let fmt_name = CStr::from_ptr((*(*fmt_ctx).iformat).name).to_string_lossy().into_owned();
            println!("格式名 = {}", fmt_name);

            let mut codecpar_list = Vec::with_capacity((*fmt_ctx).nb_streams as usize);
            let mut stream_mapping = vec![];
            for i in 0..(*fmt_ctx).nb_streams {
                let in_stream = *(*fmt_ctx).streams.add(i as usize);
                let codecpar = avcodec_parameters_alloc();

                info!("流 {}: 类型 = {}, 编解码器ID = {}, 格式 = {}, 宽度 = {}, 高度 = {}",
                    i,
                    codecpar.as_ref().map_or(0, |p| p.codec_type),
                    codecpar.as_ref().map_or(0, |p| p.codec_id),
                    codecpar.as_ref().map_or(0, |p| p.format),
                    codecpar.as_ref().map_or(0, |p| p.width),
                    codecpar.as_ref().map_or(0, |p| p.height));

                if codecpar.is_null() {
                    // 失败时回收 sdp_ptr
                    let _ = Box::from_raw(sdp_ptr);
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
                    sdp_mem_ptr: sdp_ptr,
                }),
                codecpar_list,
                stream_mapping,
            };
            Ok(ctx)
        }
    }
}

fn build_sdp(ssrc: u32, rtp_map_key: u8, rtp_map_val: &String) -> String {
    // 使用非 0 端口（例如 5004），增加 FFmpeg 解析几率
    let mut sdp = String::with_capacity(300);
    sdp.push_str("v=0\r\n");
    sdp.push_str("o=- 0 0 IN IP4 127.0.0.1\r\n");
    sdp.push_str("s=No Name\r\n");
    sdp.push_str("c=IN IP4 127.0.0.1\r\n");
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("m=video 5004 RTP/AVP {}\r\n", rtp_map_key));
    sdp.push_str(&format!("a=rtpmap:{} {}\r\n", rtp_map_key, rtp_map_val));
    sdp.push_str("a=recvonly\r\n");
    sdp.push_str(&format!("a=ssrc:{} cname:gb28181_stream\r\n", ssrc));
    sdp
}