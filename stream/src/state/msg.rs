use crate::media::context::event::ContextEvent;
use crate::media::rtp::RtpPacket;
use crate::state::layer::converter_layer::ConverterLayer;
use common::bus::mpsc::TypedReceiver;
use share::bus::media_initialize_ext::MediaExt;
use shared::info::media_info_ext::MediaExt;


pub struct StreamConfig {
    pub converter: ConverterLayer,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub media_ext: MediaExt,
    pub rtp_rx: crossbeam_channel::Receiver<RtpPacket>,
}