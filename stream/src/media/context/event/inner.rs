use base::bytes::Bytes;
use base::log::error;
use base::tokio::sync::oneshot;
use crate::media::context::format::FmtMuxer;
use crate::media::context::MediaContext;
use crate::general::mp::MediaParam;
use crate::media::context::utils::extradata;

pub enum InnerEvent {
    FlvHeader(oneshot::Sender<Bytes>),
    Mp4Header(oneshot::Sender<Bytes>),
    CmafHeader(oneshot::Sender<Bytes>),
    MediaParam(oneshot::Sender<MediaParam>),
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
                    Some(context) => {
                        if let Err(_) = sender.send(context.get_header()) {
                            error!("flv_header send to the receiver dropped");
                        }
                    }
                }
            },
            InnerEvent::Mp4Header(sender) => {
                match &media_context.muxer_context.mp4 {
                    None => {
                        error!("no mp4 context");
                    }
                    Some(context) => {
                        if let Err(_) = sender.send(context.get_header()) {
                            error!("mp4_header send to the receiver dropped");
                        }
                    }
                }
            }
            InnerEvent::CmafHeader(sender) => {
                match &media_context.muxer_context.fmp4 {
                    None => {
                        error!("no fmp4 context");
                    }
                    Some(context) => {
                        if let Err(_) = sender.send(context.get_header()) {
                            error!("fmp4_header send to the receiver dropped");
                        }
                    }
                }
            }
            InnerEvent::MediaParam(sender) => {
                let param = extradata::parse_media_param(&media_context.demuxer_context);
                if let Err(_) = sender.send(param) {
                    error!("media params send to the receiver dropped");
                }
            }
        }
    }
}