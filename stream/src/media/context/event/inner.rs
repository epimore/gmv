use base::bytes::Bytes;
use base::log::error;
use base::tokio::sync::oneshot;
use crate::media::context::format::FmtMuxer;
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
                        if let Err(_) = sender.send(fc.get_header()) {
                            error!("flv_header send to the receiver dropped");
                        }
                    }
                }
            }
        }
    }
}