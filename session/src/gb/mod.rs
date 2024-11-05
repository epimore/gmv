mod shared;
pub mod handler;

use std::net::SocketAddr;
use std::str::FromStr;
use encoding_rs::GB18030;
use rsip::{SipMessage};
use rsip::message::HeadersExt;
use common::exception::{GlobalResult, TransError};
use common::log::{debug, error};
use common::net;
use common::net::state::{Package, Zip};
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
    let (output, mut input) = net::init_net(net::state::Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
    while let Some(zip) = input.recv().await {
        debug!("receive {:?}",&zip);
        match zip {
            Zip::Data(Package { association, data }) => {
                match SipMessage::try_from(data) {
                    Ok(msg) => {
                        match msg {
                            SipMessage::Request(req) => {
                                debug!("request header:\n{}\nrequest body:\n{}",&req.headers,GB18030.decode(&req.body).0);
                                handler::requester::hand_request(req, output.clone(), &association).await?;
                            }
                            SipMessage::Response(res) => {
                                debug!("Response header:\n{}\nResponse body:\n{}",&res.headers,GB18030.decode(&res.body).0);
                                let call_id: String = res.call_id_header().hand_log(|msg| error!("{msg}"))?.clone().into();
                                let cs_eq: String = res.cseq_header().hand_log(|msg| error!("{msg}"))?.clone().into();
                                let to_device_id = parser::header::get_device_id_by_response(&res).hand_log(|msg| error!("{msg}"))?;

                                EventSession::handle_response(to_device_id, call_id, cs_eq, res).await?;
                            }
                        }
                    }
                    Err(err) => {
                        debug!("invalid data {err:?}");
                    }
                }
            }
            Zip::Event(event) => {
                if event.get_type_code() == &0u8 {
                    RWSession::clean_rw_session_by_bill(event.get_association()).await;
                }
            }
        }
    }
    Ok(())
}

fn print_start(session_conf: &SessionConf) {
    println!("Listen to gb28181 session over tcp and udp,listen ip : {} port: {}", session_conf.get_wan_ip(), session_conf.get_wan_port());
}