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

pub struct MediaContext {
    pub ssrc: u32,
    pub media_ext: MediaExt,
    pub codec_context: Option<CodecContext>,
    pub filter_context: FilterContext,
    pub muxer_context: MuxerContext,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub demuxer_context: DemuxerContext,
}
impl MediaContext {
    pub fn init(ssrc: u32, stream_config: StreamConfig) -> GlobalResult<MediaContext> {
        let rtp_buffer = RtpPacketBuffer::init(ssrc, stream_config.rtp_rx)?;
        let demuxer_context = DemuxerContext::start_demuxer(ssrc, &stream_config.media_ext, rtp_buffer)?;
        let converter = stream_config.converter;
        let context = MediaContext {
            codec_context: CodecContext::init(converter.codec),
            filter_context: FilterContext::init(converter.filter),
            ssrc,
            media_ext: stream_config.media_ext,
            context_event_rx: stream_config.context_event_rx,
            muxer_context: MuxerContext::init(&demuxer_context, converter.muxer),
            demuxer_context,
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