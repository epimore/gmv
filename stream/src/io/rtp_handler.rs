use crate::media;
use crate::state::register::Register;
use crate::state::talk::TalkManager;
use base::bytes::{Bytes, BytesMut};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, warn};
use base::net;
use base::net::rw::{PacketDispatcher, PacketSplitter, PacketWriter, U16BeLengthPrefixEncoder};
use base::net::state::{CHANNEL_BUFFER_SIZE, IoEventType, Protocol, Zip};
use base::tokio::sync::mpsc::Receiver;
use base::tokio_util::sync::CancellationToken;
use crossbeam_channel::TrySendError;
use rtp_types::RtpPacket;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;

pub fn listen_media_server(port: u16) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr =
        SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    net::listen(Protocol::ALL, socket_addr)
}

pub fn run(
    tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel: CancellationToken,
) -> GlobalResult<()> {
    let rtp_port = listener_port(&tu)?;
    let (output_tx, output_rx) = base::tokio::sync::mpsc::channel(CHANNEL_BUFFER_SIZE);
    let writer: PacketWriter<U16BeLengthPrefixEncoder> =
        net::rw::direct_rw::<RtpReader, RtpReader, U16BeLengthPrefixEncoder>(
            tu,
            cancel.clone(),
            Arc::new(RtpReader),
            Arc::new(U16BeLengthPrefixEncoder),
        )?;
    base::tokio::spawn(write_net(output_rx, writer.clone(), cancel));
    TalkManager::init_rtp_writer(writer, output_tx, rtp_port)
}

async fn write_net(
    mut output_rx: Receiver<Zip>,
    writer: PacketWriter<U16BeLengthPrefixEncoder>,
    cancel: CancellationToken,
) {
    loop {
        base::tokio::select! {
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
                            error!("rtp socket write failed: association={association:?}, err={err}");
                        }
                    }
                    Zip::Event(event) => {
                        if matches!(event.type_code, IoEventType::Close) {
                            if matches!(event.association.protocol, Protocol::ALL) {
                                break;
                            }
                            if matches!(event.association.protocol, Protocol::TCP) {
                                writer.remove_tcp_writer(&event.association.remote_addr);
                            }
                        }
                    }
                }
            }
            _ = cancel.cancelled() => break,
        }
    }
}

fn listener_port(tu: &(Option<TcpListener>, Option<UdpSocket>)) -> GlobalResult<u16> {
    if let Some(udp) = &tu.1 {
        return udp
            .local_addr()
            .map(|addr| addr.port())
            .hand_log(|msg| error!("{msg}"));
    }
    if let Some(tcp) = &tu.0 {
        return tcp
            .local_addr()
            .map(|addr| addr.port())
            .hand_log(|msg| error!("{msg}"));
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "rtp listener is empty",
        |msg| error!("{msg}"),
    ))
}

#[derive(Default)]
struct RtpReader;

impl RtpReader {
    fn forward_packet(
        &self,
        pkt: RtpPacket<'_>,
        payload: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        let ssrc = pkt.ssrc();
        let Some(rtp_tx) = Register::refresh_rtp(ssrc, pkt.payload_type(), (remote_addr, protocol))
        else {
            debug!("drop rtp packet for closed channel; ssrc: {ssrc}");
            return Ok(());
        };

        let packet = media::rtp::RtpPacket {
            ssrc,
            timestamp: pkt.timestamp(),
            marker: pkt.marker_bit(),
            seq: pkt.sequence_number(),
            payload,
        };

        match rtp_tx.try_send(packet) {
            Ok(_) => {}
            Err(TrySendError::Full(_)) => {
                warn!("rtp input channel full; drop ssrc={ssrc}");
            }
            Err(TrySendError::Disconnected(_)) => {
                debug!("drop rtp packet for disconnected channel; ssrc: {ssrc}");
            }
        }

        Ok(())
    }
}

impl PacketDispatcher for RtpReader {
    fn dispatch_owned(
        &self,
        data: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        match RtpPacket::parse(data.as_ref()) {
            Ok(pkt) => {
                let payload_start = pkt.payload_offset();
                let payload_end = payload_start + pkt.payload_len();
                let payload = data.slice(payload_start..payload_end);
                self.forward_packet(pkt, payload, remote_addr, protocol)?;
            }
            Err(error) => {
                warn!("parse rtp pkt error: {error}");
            }
        }
        Ok(())
    }
}

const TCP_RTP_HEADER_LEN: usize = 2;
const MIN_RTP_HEADER_LEN: usize = 12;
const TCP_DATA_BASE_LEN: usize = TCP_RTP_HEADER_LEN + MIN_RTP_HEADER_LEN;
const MAX_LIMIT_RTP_PACKET_SIZE: usize = 1024 * 16;

fn feed_tcp_packets<F>(buffer: &mut BytesMut, mut f: F) -> GlobalResult<()>
where
    F: FnMut(Bytes) -> GlobalResult<()>,
{
    loop {
        if buffer.len() < TCP_DATA_BASE_LEN {
            break;
        }

        let len = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;

        if len > MAX_LIMIT_RTP_PACKET_SIZE {
            buffer.clear();
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "rtp pkt size out of max limit",
                |msg| error!("{msg}: max = {MAX_LIMIT_RTP_PACKET_SIZE}, this = {len}"),
            ));
        }

        let split_len = len + TCP_RTP_HEADER_LEN;

        if buffer.len() < split_len {
            break;
        }

        let packet = buffer.split_to(split_len).freeze();
        f(packet.slice(TCP_RTP_HEADER_LEN..split_len))?;
    }
    Ok(())
}

impl PacketSplitter for RtpReader {
    fn feed_owned<F>(&mut self, buffer: &mut BytesMut, f: F) -> GlobalResult<()>
    where
        F: FnMut(Bytes) -> GlobalResult<()>,
    {
        feed_tcp_packets(buffer, f)
    }
}
