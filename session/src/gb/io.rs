use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;

use base::bytes::{Bytes, BytesMut};
use base::dashmap::DashSet;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info};
use base::net::rw::{PacketDispatcher, PacketSplitter, PacketWriter, RawPacketEncoder};
use base::net::state::{Association, Event, IoEventType, Package, Protocol, Zip};
use base::tokio;
use base::tokio::sync::mpsc::{Receiver, Sender};
use base::tokio_util::sync::CancellationToken;
use encoding_rs::GB18030;
use gmv_pjsip::SipTransmit;

pub use crate::gb::core::rw::RWContext;
use crate::gb::sip::NativeSipRuntimeHandle;

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
pub(crate) struct TcpCloseTracker {
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

pub(crate) struct NativeSessionIo {
    pub output: Sender<Zip>,
    pub input: Receiver<Zip>,
    pub writer: PacketWriter<RawPacketEncoder>,
    pub close_tracker: Arc<TcpCloseTracker>,
}

pub(crate) fn rw_by_tokio_native(
    tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel_token: CancellationToken,
) -> GlobalResult<NativeSessionIo> {
    let local_addr = listener_local_addr(&tu)?;
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(32_768);
    let (output_tx, output_rx) = tokio::sync::mpsc::channel(32_768);
    let close_tracker = Arc::new(TcpCloseTracker::default());
    let writer = base::net::rw::rw::<SipPacketDispatcher, RawChunkSplitter, RawPacketEncoder>(
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
        writer.clone(),
        close_tracker.clone(),
        cancel_token,
    ));
    Ok(NativeSessionIo {
        output: output_tx,
        input: input_rx,
        writer,
        close_tracker,
    })
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
        self.input_tx
            .try_send(Zip::build_data(Package::new(association, data)))
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
        self.input_tx
            .try_send(Zip::build_event(Event {
                association,
                type_code: IoEventType::Close,
            }))
            .hand_log(|msg| error!("session socket event channel is full: {msg}"))?;
        Ok(())
    }
}

#[derive(Default)]
struct RawChunkSplitter;

impl PacketSplitter for RawChunkSplitter {
    fn feed_owned<F>(&mut self, chunk: &mut BytesMut, mut f: F) -> GlobalResult<()>
    where
        F: FnMut(Bytes) -> GlobalResult<()>,
    {
        if chunk.is_empty() {
            return Ok(());
        }
        f(chunk.split_to(chunk.len()).freeze())
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
                let Some(zip) = item else { break; };
                match zip {
                    Zip::Data(package) => {
                        let association = package.association;
                        log_sip_payload("发送", &association, package.data.as_ref());
                        if let Err(err) = writer
                            .write_to(
                                package.data,
                                association.remote_addr,
                                association.protocol,
                            )
                            .await
                        {
                            error!(
                                "session socket write failed: association={association:?}, \
                                 err={err}"
                            );
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
                        info!(
                            "发送: event={:?}, to={:?}",
                            event.type_code, event.association
                        );
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

pub(crate) async fn write_native_net(
    mut transmits: Receiver<SipTransmit>,
    writer: PacketWriter<RawPacketEncoder>,
    runtime: NativeSipRuntimeHandle,
    close_tracker: Arc<TcpCloseTracker>,
    cancel_token: CancellationToken,
) {
    loop {
        let transmit = tokio::select! {
            transmit = transmits.recv() => transmit,
            _ = cancel_token.cancelled() => break,
        };
        let Some(transmit) = transmit else {
            break;
        };
        let protocol = match transmit.protocol {
            gmv_pjsip::SipTransportProtocol::Udp => Protocol::UDP,
            gmv_pjsip::SipTransportProtocol::Tcp => Protocol::TCP,
            gmv_pjsip::SipTransportProtocol::Tls => {
                runtime.complete_send(transmit.send_id, Err(1));
                continue;
            }
        };
        let association = Association::new(transmit.local_addr, transmit.remote_addr, protocol);
        log_sip_payload("发送", &association, &transmit.data);
        let send_id = transmit.send_id;
        let association_id = transmit.association_id;
        let sent_bytes = transmit.data.len();
        match writer
            .write_to(
                Bytes::from(transmit.data),
                association.remote_addr,
                association.protocol,
            )
            .await
        {
            Ok(()) => runtime.complete_send(send_id, Ok(sent_bytes)),
            Err(err) => {
                error!(
                    "native SIP socket write failed: send_id={}, association={association:?}, \
                     err={err}",
                    send_id
                );
                runtime.complete_send(send_id, Err(1));
                if matches!(association.protocol, Protocol::TCP) {
                    let tracked_association = runtime
                        .close_transport_id(association_id, 1)
                        .unwrap_or_else(|| association.clone());
                    if writer.has_tcp_writer(&association.remote_addr) {
                        close_tracker.mark_session_close(tracked_association.clone());
                        writer.remove_tcp_writer(&association.remote_addr);
                    }
                    handle_tcp_connection_closed(&tracked_association);
                }
            }
        }
    }
}

fn handle_tcp_connection_closed(association: &Association) {
    RWContext::clean_rw_session_by_bill(association);
}

fn is_sip_keepalive_or_empty(data: &[u8]) -> bool {
    data.is_empty()
        || data
            .iter()
            .all(|&byte| matches!(byte, b'\r' | b'\n' | b' ' | b'\t'))
}

fn compact_sip_payload(data: &[u8]) -> String {
    let payload = match std::str::from_utf8(data) {
        Ok(payload) => payload.into(),
        Err(_) => GB18030.decode(data).0,
    };
    payload.replace('\r', "").replace('\n', "\\n")
}

fn log_sip_payload(direction: &str, association: &Association, data: &[u8]) {
    debug!(
        "{direction}:{association:?} 负载: {}",
        compact_sip_payload(data)
    );
}

pub(crate) async fn read_native(
    mut input: Receiver<Zip>,
    output: Sender<Zip>,
    runtime: NativeSipRuntimeHandle,
    cancel_token: CancellationToken,
) {
    while let Some(zip) = input.recv().await {
        if cancel_token.is_cancelled() {
            break;
        }
        match zip {
            Zip::Data(Package { association, data }) => {
                log_sip_payload("接收", &association, data.as_ref());
                if is_sip_keepalive_or_empty(data.as_ref()) {
                    let _ = output
                        .send(Zip::Data(Package { association, data }))
                        .await
                        .hand_log(|msg| error!("SIP keepalive response failed: {msg}"));
                    continue;
                }
                if let Err(err) = runtime.receive_packet(association.clone(), data) {
                    error!(
                        "queue native SIP packet failed: association={association:?}, err={err}"
                    );
                }
            }
            Zip::Event(event) => {
                info!(
                    "接收: event={:?}, from={:?}",
                    event.type_code, event.association
                );
                if matches!(event.type_code, IoEventType::Close) {
                    runtime.close_transport(&event.association, 1);
                    handle_tcp_connection_closed(&event.association);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use base::bytes::BytesMut;
    use base::net::rw::PacketSplitter;
    use base::net::state::{Association, Protocol};

    use super::{RawChunkSplitter, TcpCloseSource, TcpCloseTracker, compact_sip_payload};

    #[test]
    fn sip_payload_log_is_single_line_and_reversible() {
        assert_eq!(
            compact_sip_payload(b"REGISTER sip:test SIP/2.0\r\nContent-Length: 0\r\n\r\n"),
            "REGISTER sip:test SIP/2.0\\nContent-Length: 0\\n\\n"
        );
    }

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

    #[test]
    fn raw_chunk_splitter_does_not_parse_tcp_sip() {
        let mut splitter = RawChunkSplitter;
        let mut chunk = BytesMut::from(&b"partial SIP chunk"[..]);
        let mut packets = Vec::new();
        splitter
            .feed_owned(&mut chunk, |packet| {
                packets.push(packet);
                Ok(())
            })
            .unwrap();
        assert!(chunk.is_empty());
        assert_eq!(packets, [b"partial SIP chunk".as_slice()]);
    }
}
