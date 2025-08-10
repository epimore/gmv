use encoding_rs::GB18030;
use rsip::message::HeadersExt;
use rsip::SipMessage;
use base::tokio::sync::mpsc::{Receiver, Sender};

use base::exception::{GlobalResultExt};
use base::log::{debug, error, info};
use base::net::state::{Package, Zip};
use crate::gb::handler;

use crate::gb::handler::parser;
use crate::gb::core::event::EventSession;
pub use crate::gb::core::rw::RWSession;

pub async fn read(mut input: Receiver<Zip>, output_tx: Sender<Zip>) {
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package { association, data }) => {
                match SipMessage::try_from(data) {
                    Ok(msg) => {
                        match msg {
                            SipMessage::Request(req) => {
                                info!("接收:{:?}\nRequest:\n{} {} {}\n{}\n{}",&association,&req.method,&req.uri,&req.version,&req.headers,GB18030.decode(&req.body).0);
                                let _ = handler::requester::hand_request(req, output_tx.clone(), &association).await;
                            }
                            SipMessage::Response(res) => {
                                info!("接收:{:?}\nResponse:\n{} {}\n{}\n{}",&association,&res.version,&res.status_code,&res.headers,GB18030.decode(&res.body).0);
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
                    RWSession::clean_rw_session_by_bill(event.get_association());
                }
            }
        }
    }
    info!("gb read exit");
}

pub async fn write(mut output_rx: Receiver<Zip>, output: Sender<Zip>) {
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