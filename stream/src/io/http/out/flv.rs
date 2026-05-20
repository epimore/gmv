use crate::io::http::out::{DisconnectAwareStream, OutPlayKind, stream_user_token_check};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::event::{Event, EventRes, OutEvent, OutEventRes};
use crate::state::register::{DEFAULT_EXPIRES, Register};
use axum::body::Body;
use axum::response::Response;
use base::bytes::Bytes;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::{broadcast, oneshot};
use base::tokio::time::timeout;
use futures_util::stream;
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo};
use shared::info::output::OutputEnum;
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn handler(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(
                OutputEnum::HttpFlv,
                bsi,
                stream_id.clone(),
                token.clone(),
                addr,
            )
            .await
            {
                OutPlayKind::Play => match Register::get_muxer_rx(&ssrc, MuxerEnum::Flv) {
                    Ok(rx) => {
                        let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>> =
                            Some(Box::new(move || {
                                Register::listen_output_timeout(
                                    stream_id,
                                    OutputEnum::HttpFlv,
                                    token,
                                    addr,
                                    0,
                                );
                            }));
                        send_frame(ssrc, rx, on_disconnect).await
                    }
                    Err(_) => res_404(),
                },
                OutPlayKind::Forbid => res_401(),
                OutPlayKind::Notfound => res_404(),
            }
        }
    }
}

async fn send_frame(
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    let wrapped_stream = DisconnectAwareStream {
        inner: Box::pin(flv_stream(ssrc, rx)),
        on_drop: on_disconnect,
    };

    Response::builder()
        .header("Content-Type", "video/x-flv")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(wrapped_stream))
        .unwrap()
}

enum FlvStreamState {
    Header,
    FirstKey,
    Live,
}

struct FlvStreamContext {
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
    state: FlvStreamState,
}

fn flv_stream(
    ssrc: u32,
    rx: broadcast::Receiver<Arc<MuxPacket>>,
) -> impl futures_core::Stream<Item = Result<Bytes, std::convert::Infallible>> {
    stream::unfold(
        FlvStreamContext {
            ssrc,
            rx,
            state: FlvStreamState::Header,
        },
        |mut ctx| async move {
            match ctx.state {
                FlvStreamState::Header => {
                    let header = get_header_rx(ctx.ssrc).await.ok()?;
                    ctx.state = FlvStreamState::FirstKey;
                    error!("flv header ----------");
                    Some((Ok(header), ctx))
                }
                FlvStreamState::FirstKey => {
                    let first_key = timeout(DEFAULT_EXPIRES, async {
                        loop {
                            match ctx.rx.recv().await {
                                Ok(pkt) if pkt.is_key => return Some(pkt.data.clone()),
                                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => return None,
                            }
                        }
                    })
                    .await
                    .ok()
                    .flatten()?;
                    error!("flv FirstKey ----------");
                    ctx.state = FlvStreamState::Live;
                    Some((Ok(first_key), ctx))
                }
                FlvStreamState::Live => loop {
                    match ctx.rx.recv().await {
                        Ok(pkt) => {
                            error!("flv body ----------");
                            return Some((Ok(pkt.data.clone()), ctx));
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                },
            }
        },
    )
}

async fn get_header_rx(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::FlvHeader(tx)))?;
    let header = rx.await.hand_log(|msg| error!("{msg}"))?;
    Ok(header)
}
