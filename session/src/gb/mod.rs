use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;

use encoding_rs::GB18030;
use rsip::message::HeadersExt;
use rsip::SipMessage;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use common::cfg_lib;
use common::cfg_lib::conf;
use common::serde_yaml;
use common::constructor::Get;

use common::exception::{GlobalResult, TransError};
use common::log::{debug, error, info};
use common::net;
use common::net::state::{CHANNEL_BUFFER_SIZE, Package, Zip};

use crate::gb::handler::parser;
use crate::gb::shared::event::EventSession;
pub use crate::gb::shared::rw::RWSession;

mod shared;
pub mod handler;

#[derive(Debug, Get, Deserialize)]
#[conf(prefix = "server.session")]
pub struct SessionConf {
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}

impl SessionConf {
    pub fn get_session_by_conf() -> Self {
        SessionConf::conf()
    }

    pub fn listen_gb_server(&self) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
        let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", self.get_wan_port())).hand_log(|msg| error! {"{msg}"})?;
        let res = net::sdx::listen(net::state::Protocol::ALL, socket_addr);
        info!("Listen to gb28181 session over tcp and udp,listen ip : {} port: {}", self.get_wan_ip(), self.get_wan_port());
        res
    }

    pub async fn run(tu: (Option<std::net::TcpListener>, Option<UdpSocket>)) -> GlobalResult<()> {
        let (output, input) = net::sdx::run_by_tokio(tu).await?;
        let (output_tx, output_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        let read_task = tokio::spawn(async move {
            info!("Gb server start running 111111");
            Self::read(input, output_tx).await;
        });
        let write_task = tokio::spawn(async move {
            info!("Gb server start running 2222222");
            Self::write(output_rx, output).await;
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
                                    info!("接收:{:?}\n负载:\n{}\n{}",&association,&req.headers,GB18030.decode(&req.body).0);
                                    let _ = handler::requester::hand_request(req, output_tx.clone(), &association).await;
                                }
                                SipMessage::Response(res) => {
                                    info!("接收:{:?}\n负载:\n{}\n{}",&association,&res.headers,GB18030.decode(&res.body).0);
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
                            debug!("接收:invalid data {err:?}");
                        }
                    }
                }
                Zip::Event(event) => {
                    info!("接收:event code={},from={:?}",event.type_code,event.association);
                    if event.get_type_code() == &0u8 {
                        RWSession::clean_rw_session_by_bill(event.get_association()).await;
                    }
                }
            }
        }
        info!("gb read exit");
    }

    async fn write(mut output_rx: Receiver<Zip>, output: Sender<Zip>) {
        while let Some(zip) = output_rx.recv().await {
            match &zip {
                Zip::Data(pkg) => {
                    info!("发送:{:?}\n负载:\n{}",pkg.get_association(),GB18030.decode(pkg.get_data()).0);
                }
                Zip::Event(ent) => {
                    info!("发送:{:?}\n事件code={}",ent.get_association(),ent.get_type_code());
                }
            }
            let _ = output.send(zip).await.hand_log(|msg| error!("数据发送失败:{msg}"));
        }

        info!("gb write exit");
    }
}
//
// pub async fn process_gb_server() -> GlobalResult<()> {
//     let session_conf = SessionConf::get_session_by_conf();
//     init_gb_service(&session_conf).await?;
//     error!("gb server exception:exited");
//     Ok(())
// }
//
// async fn init_gb_service(session_conf: &SessionConf) -> GlobalResult<()> {
//     let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", session_conf.get_wan_port())).hand_log(|msg| error! {"{msg}"}).expect("监听地址无效");
//     let (output, input) = net::init_net(net::state::Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
//     info!("Listen to gb28181 session over tcp and udp,listen ip : {} port: {}", session_conf.get_wan_ip(), session_conf.get_wan_port());
//     eprintln!("Listen to gb28181 session over tcp and udp,listen ip : {} port: {}", session_conf.get_wan_ip(), session_conf.get_wan_port());
//     let (output_tx, output_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
//     let read_task = tokio::spawn(async move {
//         read(input, output_tx).await;
//     });
//     let write_task = tokio::spawn(async move {
//         write(output_rx, output).await;
//     });
//     read_task.await.hand_log(|msg| error!("读取数据异常:{msg}"))?;
//     write_task.await.hand_log(|msg| error!("写出数据异常:{msg}"))?;
//     Ok(())
// }


