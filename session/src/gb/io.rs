use base::bytes::{Buf, Bytes, BytesMut};
use base::tokio::sync::mpsc::{Receiver, Sender};
use encoding_rs::GB18030;
use rsip::SipMessage;
use rsip::message::HeadersExt;

use crate::gb::handler;
use base::exception::GlobalResultExt;
use base::log::{debug, error, info};
use base::net::state::{Association, Package, Protocol, Zip};

use crate::gb::core::event::EventSession;
pub use crate::gb::core::rw::RWSession;
use crate::gb::handler::parser;
use crate::gb::sip_tcp_splitter::complete_pkt;

/// 将日志内容压缩为单行，保留可还原换行信息
fn compact_for_log(raw: &str) -> String {
    raw.replace('\r', "").replace('\n', "\\n")
}

pub async fn read(mut input: Receiver<Zip>, output_tx: Sender<Zip>) {
    let mut buffer = BytesMut::new();
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package {
                association,
                data,
            }) => {
                if let Protocol::TCP = association.protocol {
                    buffer.extend_from_slice(&data);
                    match complete_pkt(&mut buffer) {
                        None => {
                            continue;
                        }
                        Some(pks) => {
                            for data in pks {
                                hand_pkt(data, &association, output_tx.clone()).await;
                            }
                        }
                    }
                } else {
                    hand_pkt(data, &association, output_tx.clone()).await;
                }
            }
            Zip::Event(event) => {
                info!(
                    "接收: event code={}, from={:?}",
                    event.type_code, event.association
                );
                if event.get_type_code() == &0u8 {
                    RWSession::clean_rw_session_by_bill(event.get_association());
                }
            }
        }
    }
    info!("gb read exit");
}
async fn hand_pkt(data: Bytes, association: &Association, output_tx: Sender<Zip>) {
    match SipMessage::try_from(data) {
        Ok(msg) => {
            match msg {
                SipMessage::Request(req) => {
                    // 将 body 和 headers 转为单行可还原格式
                    let headers = compact_for_log(&format!("{}", &req.headers));
                    let body = compact_for_log(&GB18030.decode(&req.body).0);
                    debug!(
                        "接收:{:?} Request: {} {} {} {} {}",
                        &association, &req.method, &req.uri, &req.version, headers, body
                    );
                    let _ = handler::requester::hand_request(req, output_tx, &association).await;
                }
                SipMessage::Response(res) => {
                    let headers = compact_for_log(&format!("{}", &res.headers));
                    let body = compact_for_log(&GB18030.decode(&res.body).0);
                    debug!(
                        "接收:{:?} Response: {} {} {} {}",
                        &association, &res.version, &res.status_code, headers, body
                    );
                    match (
                        res.call_id_header(),
                        res.cseq_header(),
                        parser::header::get_device_id_by_response(&res),
                    ) {
                        (Ok(call_id), Ok(cs_eq), Ok(to_device_id)) => {
                            let _ = EventSession::handle_response(
                                to_device_id,
                                call_id.clone().into(),
                                cs_eq.clone().into(),
                                res,
                            )
                            .await;
                        }
                        (call_res, cseq_res, device_id_res) => {
                            error!(
                                "call={:?}, cseq={:?}, device_id={:?}",
                                call_res, cseq_res, device_id_res
                            );
                        }
                    }
                }
            }
        }
        Err(err) => {
            debug!("接收: invalid data {err:?}");
        }
    }
}
pub async fn write(mut output_rx: Receiver<Zip>, output: Sender<Zip>) {
    while let Some(zip) = output_rx.recv().await {
        match &zip {
            Zip::Data(pkg) => {
                let payload = compact_for_log(&GB18030.decode(pkg.get_data()).0);
                debug!("发送:{:?} 负载: {}", pkg.get_association(), payload);
            }
            Zip::Event(ent) => {
                info!(
                    "发送:{:?} 事件code={}",
                    ent.get_association(),
                    ent.get_type_code()
                );
            }
        }
        let _ = output
            .send(zip)
            .await
            .hand_log(|msg| error!("数据发送失败:{msg}"));
    }

    info!("gb write exit");
}
