use std::{pin::Pin, task::{Context, Poll}};
use std::collections::HashMap;
use std::net::SocketAddr;

use hyper::{Body, Method, Request, Response, server::accept::Accept, service::{make_service_fn, service_fn}, StatusCode};
use tokio_util::sync::CancellationToken;

use common::anyhow::{anyhow};
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{error, info};
use common::tokio::{self,
                    io::{AsyncRead, AsyncWrite, ReadBuf},
                    net::{TcpListener, TcpStream},
};
use common::tokio::sync::mpsc::Sender;

use crate::biz::{api};
use crate::biz::api::RtpMap;
use crate::general::mode::{INDEX};

const DROP_SSRC: &str = "/drop/ssrc";
const LISTEN_SSRC: &str = "/listen/ssrc";
const STOP_RECORD: &str = "/stop/record";
const START_RECORD: &str = "/start/record";
const PLAY: &str = "/play/";
const STOP_PLAY: &str = "/stop/play";
const QUERY_STATE: &str = "/query/state";
const RTP_MEDIA: &str = "/rtp/media";

async fn handle(
    node_name: &String,
    opt_addr: Option<SocketAddr>,
    req: Request<Body>,
    ssrc_tx: Sender<u32>,
    client_connection_cancel: CancellationToken,
) -> GlobalResult<Response<Body>> {
    let remote_addr = opt_addr.ok_or(SysErr(anyhow!("连接时获取客户端地址失败"))).hand_log(|msg| error!("{msg}"))?;

    match get_token(&req) {
        Ok(token) => {
            let response = biz(node_name, remote_addr, ssrc_tx, token, req, client_connection_cancel).await?;
            Ok(response)
        }
        Err(_) => {
            api::res_401()
        }
    }
}

fn get_token(req: &Request<Body>) -> GlobalResult<String> {
    let token_str = req.headers().get("gmv-token")
        .ok_or_else(|| GlobalError::new_biz_error(1100, "header无gmv-token", |msg| info!("{msg}")))?
        .to_str().hand_log(|msg| info!("获取gmv-token失败;err = {msg}"))?;
    Ok(token_str.to_string())
}

fn get_param_map(req: &Request<Body>) -> GlobalResult<HashMap<String, String>> {
    let map = form_urlencoded::parse(req.uri().query()
        .ok_or_else(|| GlobalError::new_biz_error(1100, "URL上参数不存在", |msg| error!("{msg}")))?.as_bytes())
        .into_owned()
        .collect::<HashMap<String, String>>();
    Ok(map)
}

async fn biz(node_name: &String, remote_addr: SocketAddr, ssrc_tx: Sender<u32>, token: String, req: Request<Body>, client_connection_cancel: CancellationToken) -> GlobalResult<Response<Body>> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") | (&Method::GET, "") => Ok(Response::new(Body::from(INDEX))),
        (&Method::GET, LISTEN_SSRC) => {
            match get_param_map(&req) {
                Ok(param_map) => {
                    api::listen_ssrc(param_map, ssrc_tx).await
                }
                Err(_err) => {
                    api::res_422()
                }
            }
        }
        (&Method::POST, RTP_MEDIA) => {
            match hyper::body::to_bytes(req.into_body()).await.hand_log(|msg| error!("{msg}")) {
                Ok(body_bytes) => {
                    match serde_json::from_slice::<RtpMap>(&body_bytes).hand_log(|msg| error!("{msg}")) {
                        Ok(rtp_map) => {
                            api::RtpMap::rtp_map(rtp_map)
                        }
                        Err(_) => {
                            api::res_422()
                        }
                    }
                }
                Err(_) => {
                    api::res_422()
                }
            }
        }
        (&Method::GET, DROP_SSRC) => {
            unimplemented!()
        }
        (&Method::GET, START_RECORD) => {
            unimplemented!()
        }
        (&Method::GET, STOP_RECORD) => {
            unimplemented!()
        }
        (&Method::GET, STOP_PLAY) => {
            unimplemented!()
        }
        (&Method::GET, QUERY_STATE) => {
            match req.uri().query() {
                None => { api::get_state(None) }
                Some(param) => {
                    let map = form_urlencoded::parse(param.as_bytes()).into_owned().collect::<HashMap<String, String>>();
                    api::get_state(map.get("stream_id").map(|stream_id_ref| stream_id_ref.clone()))
                }
            }
        }
        (method, uri) => {
            if method.eq(&Method::GET) {
                info!("uri = {}",uri);
                if let Some(index) = uri.rfind('.') {
                    let start_play = &format!("/{node_name}{PLAY}");
                    let p_len = start_play.len();
                    //stream_id最小20位
                    if index > p_len + 20 {
                        let play_type = &uri[index + 1..];
                        if play_type.eq("flv") || play_type.eq("m3u8") {
                            let stream_id = (&uri[p_len..index]).to_string();
                            return api::start_play(play_type.to_string(), stream_id, token, remote_addr, client_connection_cancel).await;
                        }
                    }
                }
                Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("GMV::NOTFOUND")).unwrap())
            } else {
                Ok(Response::builder().status(StatusCode::METHOD_NOT_ALLOWED).body(Body::from("GMV::METHOD_NOT_ALLOWED")).unwrap())
            }
        }
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

pub async fn run(node_name: &'static String, port: u16, tx: Sender<u32>) -> GlobalResult<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await.hand_log(|msg| error!("{msg}")).unwrap();

    let make_service = make_service_fn(|conn: &ClientConnection| {
        let opt_addr = conn.conn.peer_addr().ok();
        let client_connection_cancel = conn.cancel.clone();
        let tx_cl = tx.clone();

        async move {
            Ok::<_, GlobalError>(service_fn(move |req| {
                handle(node_name, opt_addr, req, tx_cl.clone(), client_connection_cancel.clone())
            }))
        }
    });

    hyper::server::Server::builder(ServerListener(listener)).serve(make_service).await.hand_log(|msg| error!("{msg}")).unwrap();

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