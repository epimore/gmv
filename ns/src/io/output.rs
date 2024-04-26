use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use hyper::{
    Body,
    body::Bytes,
    Request,
    Response, server::accept::Accept, service::{make_service_fn, service_fn}, StatusCode,
};
use tokio_util::sync::CancellationToken;
use common::err::{GlobalResult, TransError};
use common::log::error;

use common::tokio::{self,
                    io::{AsyncRead, AsyncWrite, ReadBuf},
                    net::{TcpListener, TcpStream},
};
use crate::general::mode::HttpStream;

async fn handle(
    req: Request<Body>,
    client_connection_cancel: CancellationToken,
) -> Result<Response<Body>, hyper::http::Error> {
    let (mut tx, rx) = Body::channel();

    // spawn background task, end when client connection is dropped
    tokio::spawn(async move {
        let mut counter = 0;
        loop {
            tokio::select! {
                _ = client_connection_cancel.cancelled() => {
                    println!("client connection is dropped, exiting loop");
                    break;
                },
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    tx.send_data(Bytes::from(format!("{counter}\n"))).await.unwrap();
                    counter += 1;
                }
            }
        }
    });

    Response::builder().status(StatusCode::OK).body(rx)
}


struct ServerListener(TcpListener);

struct ClientConnection {
    conn: TcpStream,
    cancel: CancellationToken,
}

impl Drop for ClientConnection {
    fn drop(&mut self) {
        self.cancel.cancel()
    }
}

pub async fn listen_stream(http_stream: HttpStream) -> GlobalResult<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", http_stream.get_port())).await.hand_err(|msg| error!("{msg}")).unwrap();

    let make_service = make_service_fn(|conn: &ClientConnection| {
        let client_connection_cancel = conn.cancel.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                handle(req, client_connection_cancel.clone())
            }))
        }
    });
    hyper::server::Server::builder(ServerListener(listener)).serve(make_service).await.hand_err(|msg| error!("{msg}")).unwrap();
    Ok(())
}

impl AsyncRead for ClientConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        Pin::new(&mut Pin::into_inner(self).conn).poll_read(context, buf)
    }
}

impl AsyncWrite for ClientConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut Pin::into_inner(self).conn).poll_write(context, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::into_inner(self).conn).poll_flush(context)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::into_inner(self).conn).poll_shutdown(context)
    }
}

impl Accept for ServerListener {
    type Conn = ClientConnection;

    type Error = std::io::Error;

    fn poll_accept(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Self::Conn, Self::Error>>> {
        let (conn, _addr) = futures_util::ready!(self.0.poll_accept(cx))?;
        Poll::Ready(Some(Ok(ClientConnection {
            conn,
            cancel: CancellationToken::new(),
        })))
    }
}