use std::{pin::Pin, task::{Context, Poll}};
use std::collections::HashMap;
use std::fmt::Debug;
use std::net::{SocketAddr};

use hyper::{Body, Method, Request, Response, server::accept::Accept, service::{make_service_fn, service_fn}, StatusCode};
use tokio_util::sync::CancellationToken;

use common::anyhow::{anyhow};
use common::exception::{GlobalError, GlobalResult, TransError};
use common::exception::GlobalError::SysErr;
use common::log::{error, info, warn};
use common::serde::de::DeserializeOwned;
use common::tokio::{self,
                    io::{AsyncRead, AsyncWrite, ReadBuf},
                    net::{TcpListener, TcpStream},
};
use common::tokio::sync::mpsc::Sender;

use crate::biz::{api};
use crate::biz::api::{RtpMap, SsrcLisDto};
use crate::container::PlayType;
use crate::general::mode::{INDEX};

const DROP_SSRC: &str = "/drop/ssrc";
const LISTEN_SSRC: &str = "/listen/ssrc";
const ON_RECORD: &str = "/on/record";
const PLAY: &str = "/play/";
const STOP_PLAY: &str = "/stop/play";
const QUERY_STREAM_COUNT: &str = "/query/stream/count";
const RTP_MEDIA: &str = "/rtp/media";

async fn handle(
    node_name: String,
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
    let h_res = req.headers().get("gmv-token")
        .or_else(|| req.headers().get("Gmv-Token"));
    let token = if h_res.is_none() {
        get_param_map(req)?.get("gmv-token").ok_or_else(|| GlobalError::new_biz_error(1100, "url参数获取gmv-token失败;", |msg| error!("{msg}")))?.to_string()
    } else {
        h_res.unwrap().to_str().hand_log(|msg| info!("header获取gmv-token失败;err = {msg}"))?.to_string()
    };
    Ok(token)
}

fn get_param_map(req: &Request<Body>) -> GlobalResult<HashMap<String, String>> {
    let map = form_urlencoded::parse(req.uri().query()
        .ok_or_else(|| GlobalError::new_biz_error(1100, "URL上参数不存在", |msg| error!("{msg}")))?.as_bytes())
        .into_owned()
        .collect::<HashMap<String, String>>();
    Ok(map)
}

async fn biz(node_name: String, remote_addr: SocketAddr, ssrc_tx: Sender<u32>, token: String, req: Request<Body>, client_connection_cancel: CancellationToken) -> GlobalResult<Response<Body>> {
    match req.method() {
        &Method::GET => {
            info!("Uri: {}",req.uri());
            match req.uri().path() {
                "/" | "" => {
                    Ok(Response::new(Body::from(INDEX)))
                }
                DROP_SSRC => {
                    unimplemented!()
                }
                ON_RECORD => {
                    match req.uri().query() {
                        None => { api::res_400() }
                        Some(param) => {
                            let map = form_urlencoded::parse(param.as_bytes()).into_owned().collect::<HashMap<String, String>>();
                            match api::get_ssrc(&map) {
                                Ok(ssrc) => {
                                    api::on_record(&ssrc).await
                                }
                                Err(_) => {api::res_400()}
                            }
                        }
                    }
                }
                STOP_PLAY => {
                    unimplemented!()
                }
                QUERY_STREAM_COUNT => {
                    match req.uri().query() {
                        None => { api::get_stream_count(None) }
                        Some(param) => {
                            let map = form_urlencoded::parse(param.as_bytes()).into_owned().collect::<HashMap<String, String>>();
                            api::get_stream_count(map.get("stream_id").map(|stream_id_ref| stream_id_ref))
                        }
                    }
                }
                uri => {
                    if let Some(index) = uri.rfind('.') {
                        let start_play = &format!("/{node_name}{PLAY}");
                        let p_len = start_play.len();
                        //stream_id最小20位
                        if index > p_len + 20 {
                            let play_type = &uri[index + 1..];
                            let pt = match play_type {
                                "flv" => {
                                    PlayType::Flv
                                }
                                "m3u8" => {
                                    PlayType::Hls
                                }
                                other => {
                                    error!("无效的流参数格式:{}",other);
                                    return api::res_404();
                                }
                            };
                            let stream_id = (&uri[p_len..index]).to_string();
                            return api::start_play(pt, stream_id, token, remote_addr, client_connection_cancel).await;
                        }
                    }
                    Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("GMV::NOTFOUND")).unwrap())
                }
            }
        }
        &Method::POST => {
            match req.uri().path() {
                LISTEN_SSRC => {
                    match body_to_model::<SsrcLisDto>(req).await {
                        Ok(ssrc_lis) => {
                            api::listen_ssrc(ssrc_lis)
                        }
                        Err(_) => {
                            api::res_422()
                        }
                    }
                }
                RTP_MEDIA => {
                    match body_to_model::<RtpMap>(req).await {
                        Ok(rtp_map) => {
                            api::RtpMap::rtp_map(rtp_map, ssrc_tx).await
                        }
                        Err(_) => {
                            api::res_422()
                        }
                    }
                }
                _uri => {
                    warn!("request:POST, uri = {}",req.uri());
                    Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("GMV::NOTFOUND")).unwrap())
                }
            }
        }
        &_ => {
            warn!("request:{}, uri = {}",req.method(),req.uri());
            Ok(Response::builder().status(StatusCode::METHOD_NOT_ALLOWED).body(Body::from("GMV::METHOD_NOT_ALLOWED")).unwrap())
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

pub fn listen_http_server(port: u16) -> GlobalResult<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    info!("Listen to http web addr = 0.0.0.0:{} ...", port);
    Ok(listener)
}

pub async fn run(node_name: String, std_http_listener: std::net::TcpListener, tx: Sender<u32>) -> GlobalResult<()> {
    std_http_listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
    let listener = TcpListener::from_std(std_http_listener).hand_log(|msg| error!("{msg}"))?;

    let make_service = make_service_fn(|conn: &ClientConnection| {
        let opt_addr = conn.conn.peer_addr().ok();
        let client_connection_cancel = conn.cancel.clone();
        let tx_cl = tx.clone();
        let value = node_name.clone();
        async move {
            Ok::<_, GlobalError>(service_fn(move |req| {
                handle(value.clone(), opt_addr, req, tx_cl.clone(), client_connection_cancel.clone())
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

async fn body_to_model<T>(req: Request<Body>) -> GlobalResult<T>
where
    T: DeserializeOwned + Debug,
{
    let uri = req.uri().to_string();
    let body = hyper::body::to_bytes(req.into_body()).await.hand_log(|msg| error!("{msg}"))?;
    let model = common::serde_json::from_slice::<T>(&body).hand_log(|msg| error!("{msg}"))?;
    info!("uri: {}\nbody: {:?}",uri,model);
    Ok(model)
}