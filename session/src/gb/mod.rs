mod shared;
pub mod handler;

use std::net::SocketAddr;
use std::str::FromStr;
use encoding_rs::GB18030;
use rsip::{SipMessage};
use rsip::message::HeadersExt;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use common::exception::{GlobalResult, TransError};
use common::log::{debug, error, info};
use common::net;
use common::net::state::{CHANNEL_BUFFER_SIZE, Package, Zip};
use crate::gb::handler::parser;
use crate::gb::shared::event::{EventSession};
pub use crate::gb::shared::rw::RWSession;
use crate::general::SessionConf;

pub async fn init_gb_server() {
    let session_conf = SessionConf::get_session_conf();
    let _ = run(&session_conf).await;
    error!("gb server exception:exited")
}

async fn run(session_conf: &SessionConf) -> GlobalResult<()> {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", session_conf.get_wan_port())).hand_log(|msg| error! {"{msg}"}).expect("监听地址无效");
    print_start(session_conf);
    let (output, input) = net::init_net(net::state::Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
    let (output_tx, output_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
    let read_task = tokio::spawn(async move {
        read(input, output_tx).await;
    });
    let write_task = tokio::spawn(async move {
        write(output_rx, output).await;
    });
    read_task.await.hand_log(|msg| error!("读取数据异常:{msg}"))?;
    write_task.await.hand_log(|msg| error!("写出数据异常:{msg}"))?;
    Ok(())
}

async fn read(mut input: Receiver<Zip>, output_tx: Sender<Zip>) {
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package { association, data }) => {
                match SipMessage::try_from(data) {
                    Ok(msg) => {
                        match msg {
                            SipMessage::Request(req) => {
                                info!("request header:\n{}\nrequest body:\n{}",&req.headers,GB18030.decode(&req.body).0);
                                let _ = handler::requester::hand_request(req, output_tx.clone(), &association).await;
                            }
                            SipMessage::Response(res) => {
                                info!("Response header:\n{}\nResponse body:\n{}",&res.headers,GB18030.decode(&res.body).0);
                                match (res.call_id_header(), res.cseq_header(), parser::header::get_device_id_by_response(&res)) {
                                    (Ok(call_id), Ok(cs_eq), Ok(to_device_id)) => {
                                        let _ = EventSession::handle_response(to_device_id, call_id.clone().into(), cs_eq.clone().into(), res).await;
                                    }
                                    (call_res, cseq_res, device_id_res) => {
                                        error!("call={:?},call={:?},call={:?}",call_res,cseq_res,device_id_res);
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        debug!("invalid data {err:?}");
                    }
                }
            }
            Zip::Event(event) => {
                info!("receive event code={:?},from={:?}",event.type_code,event.association);
                if event.get_type_code() == &0u8 {
                    RWSession::clean_rw_session_by_bill(event.get_association()).await;
                    // break;
                }
            }
        }
    }
}

async fn write(mut output_rx: Receiver<Zip>, output: Sender<Zip>) {
    while let Some(zip) = output_rx.recv().await {
        match &zip {
            Zip::Data(pkg) => {
                info!("发送数据: 网络组={:?},数据={:?}",pkg.get_association(),String::from_utf8(pkg.get_data().to_vec()));
            }
            Zip::Event(ent) => {
                info!("发送事件: 网络组={:?},事件code={:?}",ent.get_association(),ent.get_type_code());
            }
        }
        let _ = output.send(zip).await.hand_log(|msg| error!("数据发送失败:{msg}"));
    }
}

fn print_start(session_conf: &SessionConf) {
    println!("Listen to gb28181 session over tcp and udp,listen ip : {} port: {}", session_conf.get_wan_ip(), session_conf.get_wan_port());
}