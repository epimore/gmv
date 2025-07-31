use common::bytes::Bytes;
use common::exception::GlobalResultExt;
use common::log::error;
use common::tokio::sync::oneshot;
use crate::media::context::format::flv::FlvContext;
use crate::media::context::MediaContext;

pub enum InnerEvent {
    FlvHeader(oneshot::Sender<Bytes>),
    //...
}
impl InnerEvent {
    pub fn handle_event(self, media_context: &MediaContext) {
        match self {
            InnerEvent::FlvHeader(sender) => {
                match &media_context.muxer_context.flv {
                    None => {
                        error!("no flv context");
                    }
                    Some(fc) => {
                        let _ = sender.send(fc.flv_header.clone()).hand_log(|msg| error!("{}",msg));
                    }
                }
            }
        }
    }
}