use crate::media::context::event::ConverterEvent;
use crate::media::context::format::demuxer::DemuxerContext;
use crate::media::rtp::{RtpPacket, RtpPacketBuffer};
use crate::state::layer::converter_layer::ConverterLayer;
use common::bus::mpsc::TypedReceiver;
use common::exception::GlobalResult;
use share::bus::media_initialize_ext::MediaExt;
use crate::state::msg::StreamConfig;

pub mod event;
pub mod format;

pub struct MediaContext {
    pub ssrc: u32,
    pub media_ext: MediaExt,
    pub demuxer_context: DemuxerContext,
    pub converter: ConverterLayer,
    pub converter_event_rx: TypedReceiver<ConverterEvent>,
}
impl MediaContext {
    pub fn init(ssrc: u32, stream_config: StreamConfig) -> GlobalResult<MediaContext> {
        let rtp_buffer = RtpPacketBuffer::init(ssrc, stream_config.rtp_rx);
        let demuxer_context = DemuxerContext::start_demuxer(&stream_config.media_ext, rtp_buffer)?;
        let context = MediaContext {
            demuxer_context,
            ssrc,
            media_ext: stream_config.media_ext,
            converter: stream_config.converter,
            converter_event_rx: stream_config.converter_event_rx,
        };
        Ok(context)
    }
    
    pub fn invoke(&self) {
    }
}