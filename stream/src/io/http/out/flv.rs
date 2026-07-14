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
use base::log::{debug, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::tokio::sync::{broadcast, oneshot};
use base::tokio::time::timeout;
use futures_util::stream;
use gmv_domain::info::obj::{BaseStreamInfo, StreamPlayInfo};
use gmv_domain::info::output::OutputEnum;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

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
                        send_frame(ssrc, rx, on_disconnect)
                    }
                    Err(_) => res_404(),
                },
                OutPlayKind::Forbid => res_401(),
                OutPlayKind::Notfound => res_404(),
            }
        }
    }
}

fn send_frame(
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
        .body(Body::from_stream(wrapped_stream))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flv_response_omits_connection_header() {
        let (_, rx) = broadcast::channel(1);

        let response = send_frame(1, rx, None);

        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "video/x-flv"
        );
        assert!(response.headers().get("Connection").is_none());
    }
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
                    ctx.state = FlvStreamState::Live;
                    Some((Ok(first_key), ctx))
                }
                FlvStreamState::Live => {
                    let mut waiting_keyframe = false;
                    let mut lag_episode = FailureEpisode::default();
                    let mut lost_packets = 0u64;
                    loop {
                        match ctx.rx.recv().await {
                            Ok(pkt) if waiting_keyframe && !pkt.is_key => continue,
                            Ok(pkt) => {
                                if waiting_keyframe
                                    && let EpisodeDecision::Recovered {
                                        total,
                                        suppressed,
                                        duration,
                                    } = lag_episode.record_success(Instant::now())
                                {
                                    base::log::info!(
                                        "http flv output state changed: state=ready, previous_state=lagged, outcome=recovered, ssrc={}, lost_packets={lost_packets}, total_failures={total}, suppressed={suppressed}, duration_ms={}",
                                        ctx.ssrc,
                                        duration.as_millis()
                                    );
                                }
                                return Some((Ok(pkt.data.clone()), ctx));
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                lost_packets = lost_packets.saturating_add(skipped);
                                base::log::trace!(
                                    "http flv output lagged: stage=output, outcome=wait_keyframe, ssrc={}, lost_packets={skipped}",
                                    ctx.ssrc
                                );
                                match lag_episode.record_failure(Instant::now()) {
                                    EpisodeDecision::Started => warn!(
                                        "http flv output state changed: state=lagged, previous_state=ready, outcome=wait_keyframe, ssrc={}, lost_packets={lost_packets}",
                                        ctx.ssrc
                                    ),
                                    EpisodeDecision::Summary {
                                        total,
                                        since_last_summary,
                                        suppressed,
                                        duration,
                                    } => warn!(
                                        "http flv output remains lagged: state=lagged, outcome=ongoing, ssrc={}, lost_packets={lost_packets}, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
                                        ctx.ssrc,
                                        duration.as_millis()
                                    ),
                                    EpisodeDecision::Suppressed => {}
                                    EpisodeDecision::Recovered { .. }
                                    | EpisodeDecision::Healthy => unreachable!(),
                                }
                                waiting_keyframe = true;
                            }
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }
            }
        },
    )
}

async fn get_header_rx(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::FlvHeader(tx)))?;
    let header = rx
        .await
        .hand_log(|msg| debug!("flv header unavailable: ssrc={ssrc}, reason={msg}"))?;
    Ok(header)
}
