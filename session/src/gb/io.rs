use base::bytes::{Bytes, BytesMut};
use base::tokio::sync::mpsc::{Receiver, Sender};
use encoding_rs::GB18030;
use rsip::message::HeadersExt;
use rsip::{Error, SipMessage};
use std::sync::{Arc, LazyLock};

use crate::gb::core::event::EventSession;
pub use crate::gb::core::rw::RWSession;
use crate::gb::depot::anti::AntiReplayKind;
use crate::gb::depot::{DepotContext, SipMsg, SipPackage};
use crate::gb::handler;
use crate::gb::handler::parser;
use crate::gb::sip_tcp_splitter::complete_pkt;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::net::state::{Association, Package, Protocol, Zip};
use base::tokio_util::sync::CancellationToken;
use regex::Regex;

static R_LOG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\r\n]+").expect("Failed to compile log new line regex"));
/// 将日志内容压缩为单行，保留可还原换行信息
pub fn compact_for_log(raw: &str) -> String {
    R_LOG.replace_all(raw, "\\n").into_owned()
}
fn is_sip_keepalive_or_empty(bytes: &Bytes) -> bool {
    let data = bytes.as_ref();

    // 空数据
    if data.is_empty() {
        return true;
    }

    // 只有空白字符
    data.iter()
        .all(|&b| matches!(b, b'\r' | b'\n' | b' ' | b'\t'))
}
pub async fn read(
    mut input: Receiver<Zip>,
    output: Sender<Zip>,
    sip_pkg_tx: Sender<SipPackage>,
    cancel_token: CancellationToken,
    ctx: Arc<DepotContext>,
) {
    let mut buffer = BytesMut::new();
    while let Some(zip) = input.recv().await {
        if cancel_token.is_cancelled() {
            break;
        }
        match zip {
            Zip::Data(Package { association, data }) => {
                if is_sip_keepalive_or_empty(&data) {
                    let _ = output
                        .send(Zip::Data(Package { association, data }))
                        .await
                        .hand_log(|msg| error!("数据发送失败:{msg}"));
                    continue;
                }
                if let Protocol::TCP = association.protocol {
                    buffer.extend_from_slice(&data);
                    match complete_pkt(&mut buffer) {
                        None => {
                            continue;
                        }
                        Some(pks) => {
                            for data in pks {
                                hand_pkt(
                                    data,
                                    output.clone(),
                                    &association,
                                    sip_pkg_tx.clone(),
                                    ctx.clone(),
                                )
                                .await;
                            }
                        }
                    }
                } else {
                    hand_pkt(
                        data,
                        output.clone(),
                        &association,
                        sip_pkg_tx.clone(),
                        ctx.clone(),
                    )
                    .await;
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
}
async fn hand_pkt(
    data: Bytes,
    output: Sender<Zip>,
    association: &Association,
    sip_pkg_tx: Sender<SipPackage>,
    ctx: Arc<DepotContext>,
) {
    match SipMessage::try_from(data.clone()) {
        Ok(msg) => {
            match msg {
                SipMessage::Request(req) => {
                    // 将 body 和 headers 转为单行可还原格式
                    let headers = compact_for_log(&format!("{}", &req.headers));
                    let body = compact_for_log(&GB18030.decode(&req.body).0);
                    debug!(
                        "接收:{:?} \\nRequest: \\n{} {} {} \\n{} \\n{}\\n",
                        &association, &req.method, &req.uri, &req.version, headers, body
                    );
                    //防重放处理
                    if let Ok(true) =
                        ctx.anti_ctx
                            .process_request(&output, &req, association.clone())
                    {
                        let _ =
                            handler::requester::hand_request(req, sip_pkg_tx, &association).await;
                    }
                }
                SipMessage::Response(res) => {
                    let headers = compact_for_log(&format!("{}", &res.headers));
                    let body = compact_for_log(&GB18030.decode(&res.body).0);
                    debug!(
                        "接收:{:?} \\nResponse: {} {} \\n{} \\n{}\\n",
                        &association, &res.version, &res.status_code, headers, body
                    );
                    //事务
                    let _ = ctx.trans_ctx.handle_response(res);
                    // if let Ok(Some(res)) = ctx.trans_ctx.handle_response(res) {
                    //     match (
                    //         res.call_id_header(),
                    //         res.cseq_header(),
                    //         parser::header::get_device_id_by_response(&res),
                    //     ) {
                    //         (Ok(call_id), Ok(cs_eq), Ok(to_device_id)) => {
                    //             let _ = EventSession::handle_response(
                    //                 to_device_id,
                    //                 call_id.clone().into(),
                    //                 cs_eq.clone().into(),
                    //                 res,
                    //             )
                    //                 .await;
                    //         }
                    //         (call_res, cseq_res, device_id_res) => {
                    //             error!(
                    //             "call={:?}, cseq={:?}, device_id={:?}",
                    //             call_res, cseq_res, device_id_res
                    //         );
                    //         }
                    //     }
                    // }
                }
            }
        }
        Err(err) => {
            warn!(
                "接收: {association:?},\\n{:?} \\ninvalid data {err:?}",
                &GB18030.decode(&data).0
            );
        }
    }
}
pub async fn write(
    mut sip_pkg_rx: Receiver<SipPackage>,
    output: Sender<Zip>,
    cancel_token: CancellationToken,
    ctx: Arc<DepotContext>,
) {
    while let Some(sip_pkg) = sip_pkg_rx.recv().await {
        if cancel_token.is_cancelled() {
            let _ = output.send(Zip::build_shutdown_zip(None)).await;
            break;
        }
        match sip_pkg.sip_msg {
            SipMsg::Response(r) => {
                let data = Bytes::from(r.clone());
                if let Ok(count) = ctx
                    .anti_ctx
                    .process_response(&sip_pkg.association.remote_addr.to_string(), r)
                {
                    for _ in 0..count {
                        send_sip_pkt_out(&output, data.clone(), sip_pkg.association.clone(), None);
                    }
                    continue;
                }
                send_sip_pkt_out(&output, data, sip_pkg.association, None);
            }
            SipMsg::Request(r, ttx) => {
                if let Ok(()) =
                    ctx.trans_ctx
                        .process_request(r.clone(), sip_pkg.association.clone(), ttx)
                {
                    send_sip_pkt_out(&output, Bytes::from(r), sip_pkg.association, None);
                }
            }
        }
    }
}

pub fn send_sip_pkt_out(
    output: &Sender<Zip>,
    data: Bytes,
    association: Association,
    ext_log: Option<&str>,
) {
    let log = compact_for_log(&GB18030.decode(&data).0);
    match ext_log {
        None => {
            debug!("发送:{:?} \\n负载: {}\\n", association, log);
        }
        Some(log) => {
            debug!("[{}] 发送:{:?} \\n负载: {}\\n", log, association, log);
        }
    }
    let zip = Zip::build_data(Package::new(association, data));
    let _ = output
        .try_send(zip)
        .hand_log(|msg| error!("数据发送失败:{msg}"));
}
