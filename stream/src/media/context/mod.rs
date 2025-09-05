use crate::media::context::codec::CodecContext;
use crate::media::context::event::ContextEvent;
use crate::media::context::filter::FilterContext;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::context::format::muxer::MuxerContext;
use crate::media::rtp::RtpPacketBuffer;
use crate::state::msg::StreamConfig;
use base::bus::mpsc::TypedReceiver;
use base::exception::typed::common::MessageBusError;
use base::exception::GlobalResult;
use rsmpeg::ffi::AVPacket;
use shared::info::media_info_ext::MediaExt;

pub mod event;
pub mod format;
mod codec;
mod filter;
/// FFmpeg的AVFormatContext和AVCodecContext实例非线程安全，必须为每个线程创建独立实例
/// 通过av_lockmgr_register注册全局锁管理器，处理编解码器初始化等非线程安全操作
/// FFmpeg 6.0+默认启用pthreads支持，但仍需注意部分API（如avcodec_open2）需手动同步
#[derive(Default)]
pub struct RtpState {
    pub timestamp: u32,
    pub marker: bool,
}
pub struct MediaContext {
    pub ssrc: u32,
    pub media_ext: MediaExt,
    pub codec_context: Option<CodecContext>,
    pub filter_context: FilterContext,
    pub muxer_context: MuxerContext,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub demuxer_context: DemuxerContext,
    pub rtp_state: *mut RtpState,
}
impl Drop for MediaContext {
    fn drop(&mut self) {
        unsafe {
            if !self.rtp_state.is_null() {
                // 回收 RtpState
                drop(Box::from_raw(self.rtp_state));
                self.rtp_state = std::ptr::null_mut();
            }
        }
    }
}
impl MediaContext {
    pub fn init(ssrc: u32, stream_config: StreamConfig) -> GlobalResult<MediaContext> {
        let rtp_buffer = RtpPacketBuffer::init(ssrc, stream_config.rtp_rx)?;
        // Box → raw pointer
        let rtp_state_ptr = Box::into_raw(Box::new(RtpState::default()));
        let demuxer_context = DemuxerContext::start_demuxer(ssrc, &stream_config.media_ext, rtp_buffer, rtp_state_ptr)?;
        let converter = stream_config.converter;
        let context = MediaContext {
            codec_context: CodecContext::init(converter.codec),
            filter_context: FilterContext::init(converter.filter),
            ssrc,
            media_ext: stream_config.media_ext,
            context_event_rx: stream_config.context_event_rx,
            muxer_context: MuxerContext::init(&demuxer_context, converter.muxer),
            demuxer_context,
            rtp_state: rtp_state_ptr,
        };
        Ok(context)
    }

    pub fn invoke(&mut self) {
        unsafe {
            let fmt_ctx = self.demuxer_context.avio.fmt_ctx;
            let mut pkt = std::mem::zeroed::<AVPacket>();
            loop {
                // 处理converter事件
                match self.context_event_rx.try_recv() {
                    Ok(event) => { self.handle_event(event) }
                    Err(MessageBusError::ChannelClosed) => { break; }
                    Err(_) => {}
                }

                let ret = rsmpeg::ffi::av_read_frame(fmt_ctx, &mut pkt);
                if ret < 0 {
                    break;
                }
                // ---- RTP timestamp / marker ----
                let rtp_ts = (*self.rtp_state).timestamp;

                // ---- 转换 RTP ts → AVStream 时基 ----
                let tb = (*(*fmt_ctx)
                    .streams
                    .offset(pkt.stream_index as isize)
                    .read())
                    .time_base;

                pkt.dts = rsmpeg::ffi::av_rescale_q(
                    rtp_ts as i64,
                    rsmpeg::ffi::AVRational { num: 1, den: 90000 }, // RTP 视频时间基
                    tb,
                );
                pkt.pts = pkt.dts;

                // 暂不实现处理codec
                // &mut self.codec_context.as_mut().map(|cc|Self::handle_codec(cc));
                // 暂不实现处理filter
                // Self::handle_filter(&mut self.filter_context);
                // 处理muxer
                Self::handle_muxer(&mut self.muxer_context, &pkt);

                rsmpeg::ffi::av_packet_unref(&mut pkt);
            }
        }
    }

    fn handle_codec(codec: &mut CodecContext) {}
    fn handle_filter(filter: &mut FilterContext) {}

    fn handle_muxer(muxer: &mut MuxerContext, pkt: &AVPacket) {
        if let Some(flv_context) = &mut muxer.flv {
            flv_context.write_packet(pkt);
        }
        if let Some(mpr_context) = &muxer.mp4 { unimplemented!() }
        if let Some(ts_context) = &muxer.ts { unimplemented!() }
        if let Some(rtp_frame_context) = &muxer.rtp_frame { unimplemented!() }
        if let Some(rtp_ps_context) = &muxer.rtp_ps { unimplemented!() }
        if let Some(rtp_enc_context) = &muxer.rtp_enc { unimplemented!() }
        if let Some(frame_context) = &muxer.frame { unimplemented!() }
    }

    fn handle_event(&mut self, event: ContextEvent) {
        match event {
            ContextEvent::Codec(_) => { unimplemented!() }
            ContextEvent::Muxer(m_event) => {
                m_event.handle_event(&mut self.muxer_context, &self.demuxer_context);
            }
            ContextEvent::Filter(_) => { unimplemented!() }
            ContextEvent::Inner(i_event) => {
                i_event.handle_event(&self);
            }
        }
    }
}