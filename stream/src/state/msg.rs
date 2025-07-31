use common::bus::mpsc::TypedReceiver;
use common::bytes::Bytes;
use common::constructor::New;
use common::serde::Serialize;
use common::tokio::sync::broadcast;
use share::bus::media_initialize_ext::MediaExt;
use crate::media::context::event::ConverterEvent;
use crate::media::rtp::RtpPacket;
use crate::state::layer::converter_layer::ConverterLayer;
// eg:rtpmap:96 H264/90000
// pub struct SdpMsg {
//     //96
//     pub fmt_code: u8,
//     //H264/90000
//     pub fmt_val: String,
// }

//todo 其他消息通道
// pub enum MuxerSender {
//     Flv(broadcast::Sender<(bool, Bytes)>),
//     Hls(broadcast::Sender<Bytes>),
// }


pub struct StreamConfig {
    pub converter: ConverterLayer,
    pub converter_event_rx: TypedReceiver<ConverterEvent>,
    pub media_ext: MediaExt,
    pub rtp_rx: crossbeam_channel::Receiver<RtpPacket>,
}