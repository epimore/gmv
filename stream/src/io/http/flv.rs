use crate::io::hook_handler::{OutEvent, OutEventRes};
use crate::io::http::{res_401, res_404, DisconnectAwareStream};
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::event::ContextEvent;
use crate::media::context::format::flv::FlvPacket;
use crate::state::{cache, TIME_OUT};
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query};
use axum::response::Response;
use axum::Router;
use common::bytes::Bytes;
use common::exception::{GlobalResult, GlobalResultExt};
use common::log::error;
use common::tokio::sync::{broadcast, oneshot};
use common::tokio::time::timeout;
use futures_core::Stream;
use shared::info::obj::{HttpStreamType, StreamPlayInfo};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use futures_util::{stream, StreamExt};

pub fn routes(node: &String) -> Router {
    Router::new()
        .nest(
            &format!("/{}", node),
            Router::new().route("/play/{stream_id}.flv", axum::routing::get(flv_handler)),
        )
}
async fn flv_handler(Path(stream_id): Path<String>, Query(token): Query<Option<String>>, ConnectInfo(addr): ConnectInfo<SocketAddr>)
                     -> Response<Body> {
    if token.is_none() {
        return res_401();
    }
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => {
            res_404()
        }
        Some((bsi, user_count)) => {
            let ssrc = bsi.rtp_info.ssrc;
            let remote_addr = addr.to_string();
            let info = StreamPlayInfo::new(bsi, remote_addr.clone(), token.clone().unwrap(), HttpStreamType::HttpFlv, user_count);
            let (tx, rx) = oneshot::channel();
            let event_tx = cache::get_event_tx();
            let _ = event_tx.send((OutEvent::OnPlay(info), Some(tx))).await.hand_log(|msg| error!("{msg}"));
            match rx.await.hand_log(|msg| error!("{msg}")) {
                Ok(OutEventRes::OnPlay(Some(true))) => {
                    match cache::get_flv_rx(&ssrc) {
                        Ok(rx) => {
                            let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync + 'static>> = Some(Box::new(move || {
                                if let Some((bsi, user_count)) = cache::get_base_stream_info_by_stream_id(&stream_id) {
                                    let info = StreamPlayInfo::new(bsi, remote_addr, token.unwrap(), HttpStreamType::HttpFlv, user_count);
                                    let _ = event_tx.try_send((OutEvent::OffPlay(info), None)).hand_log(|msg| error!("{msg}"));
                                }
                            }));
                            send_frame(ssrc, rx, on_disconnect).await
                        }
                        Err(_) => { res_404() }
                    }
                }
                Ok(_) => res_401(),
                Err(_) => {
                    //对端关闭,表示流已释放
                    res_404()
                }
            }
        }
    }
}
async fn send_frame(
    ssrc: u32,
    mut rx: broadcast::Receiver<Arc<FlvPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    // 获取 header
    let header = match get_header_rx(ssrc).await {
        Ok(h) => h,
        Err(_) => return res_404(),
    };

    // 等待第一个关键帧
    let first_key = match timeout(Duration::from_millis(TIME_OUT), async {
        loop {
            match rx.recv().await {
                Ok(pkt) if pkt.is_key => break Some(pkt.data.clone()),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }).await {
        Ok(Some(data)) => data,
        _ => return res_404(),
    };

    // 构建数据流：header -> 首帧关键帧 -> 实时流
    let header_stream = stream::once(Box::pin(async move { Ok::<_, std::convert::Infallible>(header) }));
    let first_key_stream = stream::once(Box::pin(async move { Ok::<_, std::convert::Infallible>(first_key) }));
    let live_stream = FlvStream { rx };

    // 包装为 disconnect aware
    let full_stream = header_stream.chain(first_key_stream).chain(live_stream);

    let wrapped_stream = DisconnectAwareStream {
        inner: full_stream,
        on_drop: on_disconnect,
    };

    Response::builder()
        .header("Content-Type", "video/x-flv")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(wrapped_stream))
        .unwrap()
}

async fn get_header_rx(ssrc: u32) -> GlobalResult<Bytes> {
    let (tx, rx) = oneshot::channel();
    cache::try_publish_mpsc(&ssrc, ContextEvent::Inner(InnerEvent::FlvHeader(tx)))?;
    let header = rx.await.hand_log(|msg| error!("{msg}"))?;
    Ok(header)
}

pub struct FlvStream {
    rx: broadcast::Receiver<Arc<FlvPacket>>,
}

impl Stream for FlvStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.rx).poll_recv(cx) {
            Poll::Ready(Some(pkt)) => Poll::Ready(Some(Ok(pkt.data.clone()))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

