mod shard;
pub mod handler;

use std::net::SocketAddr;
use std::str::FromStr;
use rsip::{Error, SipMessage};
use rsip::message::HeadersExt;
use common::conf::get_config;
use common::err::{GlobalResult, TransError};
use common::log::{debug, error, warn};
use common::net;
use common::net::shard::{Bill, Event, Package, Zip};
use common::tokio::sync::mpsc::{Receiver, Sender};
use crate::gb::handler::parser;
use crate::gb::shard::event::{EventSession, Ident};
use crate::gb::shard::rw::RWSession;
use crate::the_common::SessionConf;

pub async fn gb_run(session_conf: &SessionConf) -> GlobalResult<()> {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", session_conf.get_wan_port())).hand_err(|msg| error! {"{msg}"}).expect("监听地址无效");
    let (output, mut input) = net::init_net(net::shard::Protocol::ALL, socket_addr).await.hand_err(|msg| error!("{msg}")).expect("网络监听失败");
    while let Some(zip) = input.recv().await {
        debug!("receive {:?}",&zip);
        let bill = zip.get_bill();
        match zip {
            Zip::Data(package) => {
                match SipMessage::try_from(package.get_owned_data()) {
                    Ok(msg) => {
                        match msg {
                            SipMessage::Request(req) => {
                                handler::requester::hand_request(req, output.clone(), &bill).await?;
                            }
                            SipMessage::Response(res) => {
                                let call_id: String = res.call_id_header().hand_err(|msg| error!("{msg}"))?.clone().into();
                                let cs_eq: String = res.cseq_header().hand_err(|msg| error!("{msg}"))?.clone().into();
                                //todo 是否固定为device_id;play时需测试
                                let device_id = parser::header::get_device_id_by_response(&res)?;
                                let ident = Ident::new(device_id, call_id, cs_eq);
                                EventSession::handle_event(&ident, res).await?;
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
                    RWSession::clean_rw_session_by_bill(event.get_bill())?;
                }
            }
        }
    }
    Ok(())
}