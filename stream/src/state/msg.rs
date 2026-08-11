use crate::media::context::event::ContextEvent;
use crate::media::rtp::RtpPacket;
use crate::state::layer::converter_layer::ConverterLayer;
use base::bus::mpsc::TypedReceiver;
use gmv_domain::info::media_info_ext::MediaExt;
use std::time::{Duration, Instant};

pub struct StreamConfig {
    pub converter: ConverterLayer,
    pub context_event_rx: TypedReceiver<ContextEvent>,
    pub media_ext: MediaExt,
    pub rtp_rx: crossbeam_channel::Receiver<RtpPacket>,
    pub first_rtp_at: Instant,
    pub startup_io_deadline: Instant,
    pub resolved_in_wait_timeout: Duration,
}
