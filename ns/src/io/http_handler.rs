use std::{convert::Infallible, io, pin::Pin, result, task::{Context, Poll}, time::Duration};
use std::collections::HashMap;
use std::io::Error;
use std::net::SocketAddr;

use hyper::{Body, body::Bytes, Method, Request, Response, server::accept::Accept, service::{make_service_fn, service_fn}, StatusCode};
use tokio_util::sync::CancellationToken;

use common::anyhow::anyhow;
use common::err::{BizError, GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{debug, error, info, warn};
use common::tokio::{self,
                    io::{AsyncRead, AsyncWrite, ReadBuf},
                    net::{TcpListener, TcpStream},
};
use common::tokio::sync::mpsc::Sender;
use common::tokio::sync::oneshot;

use crate::biz;
use crate::biz::{api, call};
use crate::general::mode::{INDEX, ResMsg, ServerConf};

async fn handle(
    opt_addr: Option<SocketAddr>,
    req: Request<Body>,
    ssrc_tx: Sender<u32>,
    client_connection_cancel: CancellationToken,
) -> GlobalResult<Response<Body>> {
    
    let remote_addr = opt_addr.ok_or(SysErr(anyhow!("连接时获取客户端地址失败")))?;
    
    match get_token(&req) {
        Ok(token) => {
            tokio::spawn(async move {
                client_connection_cancel.cancelled().await;
                println!("close.....");
                //todo callback off_play by token..
                //call::StreamPlayInfo::off_play();
            });
            
            let response = biz(remote_addr, ssrc_tx, token, req).await?;
            Ok(response)
        }
        Err(_) => {
            api::res_401()
        }
    }
}

fn get_token(req: &Request<Body>) -> GlobalResult<String> {
    let token_str = req.headers().get("gmv-token")
        .ok_or_else(||GlobalError::new_biz_error(1100, "header无gmv-token", |msg| info!("{msg}")))?
        .to_str().hand_err(|msg| info!("获取gmv-token失败;err = {msg}"))?;
    Ok(token_str.to_string())
}

async fn biz(remote_addr: SocketAddr, ssrc_tx: Sender<u32>, token: String, req: Request<Body>) -> GlobalResult<Response<Body>> {
    
    // let (tx, rx) = tokio::sync::broadcast::channel(100);
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") | (&Method::GET, "") => Ok(Response::new(Body::from(INDEX))),
        (&Method::GET, "/listen/ssrc") => {
            biz::api::listen_ssrc(&req, ssrc_tx).await
        }
        (&Method::GET, "/drop/ssrc") => {
            unimplemented!()
        }
        (&Method::GET, "/start/record") => {
            unimplemented!()
        }
        (&Method::GET, "/stop/record") => {
            unimplemented!()
        }
        (&Method::GET, "/start/play") => {
            unimplemented!()
        }
        (&Method::GET, "/stop/play") => {
            unimplemented!()
        }
        (&Method::GET, "/query/state") => {
            unimplemented!()
        }
        _ => Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("GMV::NOTFOUND")).unwrap()),
    }
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

pub async fn run(port: u16, tx: Sender<u32>) -> GlobalResult<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await.hand_err(|msg| error!("{msg}")).unwrap();
    
    let make_service = make_service_fn(|conn: &ClientConnection| {
        
        let opt_addr = conn.conn.peer_addr().ok();
        let client_connection_cancel = conn.cancel.clone();
        let tx_cl = tx.clone();
        
        async move {
            Ok::<_, GlobalError>(service_fn(move |req| {
                
                handle(opt_addr, req, tx_cl.clone(), client_connection_cancel.clone())
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