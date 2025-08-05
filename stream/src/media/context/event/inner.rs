use common::bytes::Bytes;
use common::exception::GlobalResultExt;
use common::log::error;
use common::tokio::sync::oneshot;
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
                        if let Err(_) = sender.send(fc.flv_header.clone()) {
                            error!("flv_header send to the receiver dropped");
                        }
                    }
                }
            }
        }
    }
}