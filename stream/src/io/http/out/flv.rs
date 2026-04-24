use crate::io::http::out::{stream_user_token_check, DisconnectAwareStream, OutPlayKind};
use crate::io::http::{res_401, res_404};
use crate::media::context::event::ContextEvent;
use crate::media::context::event::inner::InnerEvent;
use crate::media::context::format::MuxPacket;
use crate::media::context::format::muxer::MuxerEnum;
use crate::state::event::{Event, EventRes, OutEvent, OutEventRes};
use crate::state::register::{Register, DEFAULT_EXPIRES};
use axum::body::Body;
use axum::response::Response;
use base::bytes::Bytes;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::tokio::sync::{broadcast, oneshot};
use base::tokio::time::timeout;
use futures_core::Stream;
use futures_util::{StreamExt, stream};
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo};
use shared::info::output::OutputEnum;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub async fn handler(stream_id: Arc<str>, token: Arc<str>, addr: SocketAddr) -> Response<Body> {
    match Register::get_base_stream_info_by_stream_id(stream_id.clone()) {
        None => res_404(),
        Some(bsi) => {
            let ssrc = bsi.rtp_info.ssrc;
            match stream_user_token_check(OutputEnum::HttpFlv,bsi,stream_id.clone(),token.clone(),addr).await {
                OutPlayKind::Play => {
                    match Register::get_muxer_rx(&ssrc, MuxerEnum::Flv) {
                        Ok(rx) => {
                            let on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>> =
                                Some(Box::new(move || {
                                    Register::listen_output_timeout(
                                        stream_id,
                                        OutputEnum::HttpFlv,
                                        token,
                                        addr,
                                        0
                                    );
                                }));
                            send_frame(ssrc, rx, on_disconnect).await
                        }
                        Err(_) => res_404(),
                    }
                }
                OutPlayKind::Forbid => {res_401()}
                OutPlayKind::Notfound => {res_404()}
            }
        }
    }
}
async fn send_frame(
    ssrc: u32,
    mut rx: broadcast::Receiver<Arc<MuxPacket>>,
    on_disconnect: Option<Box<dyn FnOnce() + Send + Sync>>,
) -> Response<Body> {
    // 获取 header
    let header = match get_header_rx(ssrc).await {
        Ok(h) => h,
        Err(_) => return res_404(),
    };
    // 等待第一个关键帧
    let first_key = match timeout(DEFAULT_EXPIRES, async {
        loop {
            match rx.recv().await {
                Ok(pkt) if pkt.is_key => break Some(pkt.data.clone()),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    })
    .await
    {
        Ok(Some(data)) => data,
        _ => return res_404(),
    };

    // 构建数据流：header -> 首帧关键帧 -> 实时流
    let header_stream = stream::once(Box::pin(async move {
        Ok::<_, std::convert::Infallible>(header)
    }));
    let first_key_stream = stream::once(Box::pin(async move {
        Ok::<_, std::convert::Infallible>(first_key)
    }));
    let live_stream = FlvStream {
        inner: BroadcastStream::new(rx),
    };

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
    Register::try_publish_mpsc(ssrc, ContextEvent::Inner(InnerEvent::FlvHeader(tx)))?;
    let header = rx.await.hand_log(|msg| error!("{msg}"))?;
    Ok(header)
}

struct FlvStream {
    inner: BroadcastStream<Arc<MuxPacket>>,
}

impl Stream for FlvStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(pkt))) => Poll::Ready(Some(Ok(pkt.data.clone()))),
            Poll::Ready(Some(Err(_))) => Poll::Pending, // broadcast lagged, skip
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
