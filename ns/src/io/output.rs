use std::{convert::Infallible, io, pin::Pin, result, task::{Context, Poll}, time::Duration};
use std::f32::consts::E;
use std::io::Error;
use std::net::SocketAddr;

use hyper::{
    Body,
    body::Bytes,
    Request,
    Response, server::accept::Accept, service::{make_service_fn, service_fn}, StatusCode,
};
use tokio_util::sync::CancellationToken;

use common::anyhow::anyhow;
use common::err::{BizError, GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{debug, error, warn};
use common::tokio::{self,
                    io::{AsyncRead, AsyncWrite, ReadBuf},
                    net::{TcpListener, TcpStream},
};

use crate::general::mode::HttpStream;

async fn handle(
    opt_addr: Option<SocketAddr>,
    req: Request<Body>,
    client_connection_cancel: CancellationToken,
) -> GlobalResult<Response<Body>> {
// ) -> Result<Response<Body>, hyper::http::Error> {
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

    let response = Response::builder().status(StatusCode::OK).body(rx).hand_err(|msg| error!("{msg}"))?;
    Ok(response)
}

/// HTTP status code 404
async fn req_bad() -> GlobalResult<Response<Body>> {
    let res = Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::empty())
        .hand_err(|msg| warn!("{msg}"))?;
    Ok(res)
}

async fn biz(remote_addr: SocketAddr,
             req: Request<Body>) {
    // let (tx, rx) = tokio::sync::broadcast::channel(100);
    unimplemented!()
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
        let opt_addr = conn.conn.peer_addr().ok();
        let client_connection_cancel = conn.cancel.clone();
        async move {
            Ok::<_, GlobalError>(service_fn(move |req| {
                handle(opt_addr, req, client_connection_cancel.clone())
            }))
        }
        /*match conn.conn.peer_addr() {
            Ok(remote_addr) => {
                async move {
                    Ok::<_, GlobalError>(service_fn(move |req| {
                        handle(remote_addr, req, client_connection_cancel.clone())
                    }))
                }
            }
            Err(err) => {
                async move {
                    Ok::<_, GlobalError>(service_fn(move |_req| {
                        // debug!("连接时获取客户端地址失败,{err}");
                        req_bad()
                    }))
                }
            }
        }*/
        // let remote_addr = conn.conn.peer_addr().hand_err(|err|warn!("获取客户端地址失败,err={}",err))?;
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