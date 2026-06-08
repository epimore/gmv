use base::bytes::{Bytes, BytesMut};
use base::dashmap::{DashMap, DashSet};
use base::tokio::sync::Mutex as AsyncMutex;
use base::tokio::sync::mpsc::{Receiver, Sender};
use encoding_rs::GB18030;
use rsip::SipMessage;
use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;

pub use crate::gb::core::rw::RWContext;
use crate::gb::depot::trans::TransactionContext;
use crate::gb::depot::{DepotContext, SipMsg, SipPackage};
use crate::gb::handler;
use crate::gb::sip_tcp_splitter::complete_pkt;
use base::err::BaseErrorCode;
use base::exception::GlobalResultExt;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error, info, warn};
use base::net::rw::{PacketDispatcher, PacketSplitter, PacketWriter, RawPacketEncoder};
use base::net::state::{
    Association, CHANNEL_BUFFER_SIZE, Event, IoEventType, Package, Protocol, Zip,
};
use base::tokio;
use base::tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TcpCloseSource {
    SessionActive,
    SessionShutdown,
    PeerOrNetwork,
}

impl TcpCloseSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::SessionActive => "session_active",
            Self::SessionShutdown => "session_shutdown",
            Self::PeerOrNetwork => "device_or_network",
        }
    }
}

#[derive(Default)]
struct TcpCloseTracker {
    session_closes: DashSet<Association>,
}

impl TcpCloseTracker {
    fn mark_session_close(&self, association: Association) {
        self.session_closes.insert(association);
    }

    fn take_source(&self, association: &Association) -> TcpCloseSource {
        if self.session_closes.remove(association).is_some() {
            TcpCloseSource::SessionActive
        } else {
            TcpCloseSource::PeerOrNetwork
        }
    }
}

pub fn rw_by_tokio(
    tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel_token: CancellationToken,
) -> GlobalResult<(Sender<Zip>, Receiver<Zip>)> {
    let local_addr = listener_local_addr(&tu)?;
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(CHANNEL_BUFFER_SIZE);
    let (output_tx, output_rx) = tokio::sync::mpsc::channel(CHANNEL_BUFFER_SIZE);
    let close_tracker = Arc::new(TcpCloseTracker::default());
    let writer = base::net::rw::rw::<SipPacketDispatcher, SipPacketSplitter, RawPacketEncoder>(
        tu,
        cancel_token.clone(),
        Arc::new(SipPacketDispatcher {
            local_addr,
            input_tx,
            close_tracker: close_tracker.clone(),
            cancel_token: cancel_token.clone(),
        }),
        Arc::new(RawPacketEncoder),
    )?;
    tokio::spawn(write_net(
        output_rx,
        writer,
        close_tracker,
        cancel_token,
    ));
    Ok((output_tx, input_rx))
}

fn listener_local_addr(tu: &(Option<TcpListener>, Option<UdpSocket>)) -> GlobalResult<SocketAddr> {
    if let Some(udp) = &tu.1 {
        return udp
            .local_addr()
            .hand_log(|msg| error!("session udp local addr failed: {msg}"));
    }
    if let Some(tcp) = &tu.0 {
        return tcp
            .local_addr()
            .hand_log(|msg| error!("session tcp local addr failed: {msg}"));
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "session listener is empty",
        |msg| error!("{msg}"),
    ))
}

struct SipPacketDispatcher {
    local_addr: SocketAddr,
    input_tx: Sender<Zip>,
    close_tracker: Arc<TcpCloseTracker>,
    cancel_token: CancellationToken,
}

impl PacketDispatcher for SipPacketDispatcher {
    fn dispatch_owned(
        &self,
        data: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        let association = Association::new(self.local_addr, remote_addr, protocol);
        let zip = Zip::build_data(Package::new(association, data));
        self.input_tx
            .try_send(zip)
            .hand_log(|msg| error!("session socket input channel is full: {msg}"))?;
        Ok(())
    }

    fn close(&self, remote_addr: SocketAddr, protocol: Protocol) -> GlobalResult<()> {
        let association = Association::new(self.local_addr, remote_addr, protocol);
        if matches!(protocol, Protocol::TCP) {
            let mut source = self.close_tracker.take_source(&association);
            if source == TcpCloseSource::PeerOrNetwork && self.cancel_token.is_cancelled() {
                source = TcpCloseSource::SessionShutdown;
            }
            debug!(
                "tcp disconnected: source={}, association={association:?}",
                source.as_str()
            );
        }
        let event = Event {
            association,
            type_code: IoEventType::Close,
        };
        self.input_tx
            .try_send(Zip::build_event(event))
            .hand_log(|msg| error!("session socket event channel is full: {msg}"))?;
        Ok(())
    }
}

#[derive(Default)]
struct SipPacketSplitter {
    packets: VecDeque<Bytes>,
}

impl PacketSplitter for SipPacketSplitter {
    fn feed_owned<F>(&mut self, chunk: &mut BytesMut, mut f: F) -> GlobalResult<()>
    where
        F: FnMut(Bytes) -> GlobalResult<()>,
    {
        if chunk.is_empty() {
            return Ok(());
        }
        if is_sip_keepalive_or_empty(chunk.as_ref()) {
            let packet = chunk.split_to(chunk.len()).freeze();
            f(packet)?;
            return Ok(());
        }

        self.packets.clear();
        complete_pkt(chunk, &mut self.packets);
        while let Some(packet) = self.packets.pop_front() {
            f(packet)?;
        }

        Ok(())
    }
}

async fn write_net(
    mut output_rx: Receiver<Zip>,
    writer: PacketWriter<RawPacketEncoder>,
    close_tracker: Arc<TcpCloseTracker>,
    cancel_token: CancellationToken,
) {
    loop {
        tokio::select! {
            item = output_rx.recv() => {
                let Some(zip) = item else {
                    break;
                };
                match zip {
                    Zip::Data(package) => {
                        let association = package.association;
                        if let Err(err) = writer
                            .write_to(package.data, association.remote_addr, association.protocol)
                            .await
                        {
                            error!("session socket write failed: association={association:?}, err={err}");
                            if matches!(association.protocol, Protocol::TCP) {
                                debug!(
                                    "tcp disconnected: source=write_failure, \
                                     association={association:?}, err={err}"
                                );
                                writer.remove_tcp_writer(&association.remote_addr);
                                handle_tcp_connection_closed(&association);
                            }
                        }
                    }
                    Zip::Event(event) => {
                        if matches!(event.type_code, IoEventType::Close) {
                            if matches!(event.association.protocol, Protocol::ALL) {
                                break;
                            }
                            if matches!(event.association.protocol, Protocol::TCP) {
                                debug!(
                                    "tcp disconnect requested: source=session_active, \
                                     association={:?}",
                                    event.association
                                );
                                if writer.has_tcp_writer(&event.association.remote_addr) {
                                    close_tracker.mark_session_close(event.association.clone());
                                    writer.remove_tcp_writer(&event.association.remote_addr);
                                } else {
                                    debug!(
                                        "tcp disconnected: source=session_active, \
                                         association={:?}, writer=absent",
                                        event.association
                                    );
                                    handle_tcp_connection_closed(&event.association);
                                }
                            }
                        }
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                debug!("session network io shutdown requested");
                break;
            },
        }
    }
}

fn handle_tcp_connection_closed(association: &Association) {
    RWContext::clean_rw_session_by_bill(association);
    TransactionContext::handle_connection_closed(association);
}

/// 将日志内容压缩为单行，保留可还原换行信息
pub fn compact_for_log(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() * 2);
    for c in raw.chars() {
        match c {
            '\r' => (), // 忽略回车
            '\n' => result.push_str("\\n"),
            _ => result.push(c),
        }
    }
    result
}
fn is_sip_keepalive_or_empty(data: &[u8]) -> bool {
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
    let request_locks: Arc<DashMap<Association, Arc<AsyncMutex<()>>>> = Arc::new(DashMap::new());
    while let Some(zip) = input.recv().await {
        if cancel_token.is_cancelled() {
            break;
        }
        match zip {
            Zip::Data(Package { association, data }) => {
                if is_sip_keepalive_or_empty(data.as_ref()) {
                    let _ = output
                        .send(Zip::Data(Package { association, data }))
                        .await
                        .hand_log(|msg| error!("数据发送失败:{msg}"));
                    continue;
                }
                hand_pkt(
                    data,
                    output.clone(),
                    &association,
                    sip_pkg_tx.clone(),
                    ctx.clone(),
                    request_locks.clone(),
                )
                .await;
            }
            Zip::Event(event) => {
                debug!(
                    "接收: event code={:?}, from={:?}",
                    event.type_code, event.association
                );
                if matches!(event.type_code, IoEventType::Close) {
                    request_locks.remove(&event.association);
                    handle_tcp_connection_closed(&event.association);
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
    request_locks: Arc<DashMap<Association, Arc<AsyncMutex<()>>>>,
) {
    match SipMessage::try_from(data) {
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
                        let association = association.clone();
                        let request_lock = request_locks
                            .entry(association.clone())
                            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                            .clone();
                        tokio::spawn(async move {
                            let _guard = request_lock.lock().await;
                            let _ = handler::requester::hand_request(req, sip_pkg_tx, association)
                                .await;
                        });
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
                }
            }
        }
        Err(err) => {
            warn!("接收: {association:?},\\n invalid data {err:?}",);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TcpCloseSource, TcpCloseTracker};
    use base::net::state::{Association, Protocol};

    #[test]
    fn tcp_close_tracker_distinguishes_session_and_peer_closes() {
        let tracker = TcpCloseTracker::default();
        let association = Association::new(
            "0.0.0.0:25600".parse().unwrap(),
            "171.217.40.25:50267".parse().unwrap(),
            Protocol::TCP,
        );

        tracker.mark_session_close(association.clone());
        assert_eq!(
            tracker.take_source(&association),
            TcpCloseSource::SessionActive
        );
        assert_eq!(
            tracker.take_source(&association),
            TcpCloseSource::PeerOrNetwork
        );
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
            SipMsg::Request(r, cb) => {
                if let Ok(()) =
                    ctx.trans_ctx
                        .process_request(r.clone(), sip_pkg.association.clone(), cb)
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
            debug!("发送:{:?} \\n{}\\n", association, log);
        }
        Some(p_log) => {
            debug!("[{}] 发送:{:?} \\n{}\\n", p_log, association, log);
        }
    }
    let zip = Zip::build_data(Package::new(association, data));
    let _ = output
        .try_send(zip)
        .hand_log(|msg| error!("数据发送失败:{msg}"));
}
