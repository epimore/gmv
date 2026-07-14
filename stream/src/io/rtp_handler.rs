use crate::io::talk::TalkManager;
use crate::media;
use crate::state::register::{RefreshRtp, Register};
use base::bytes::{Bytes, BytesMut};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::net;
use base::net::rw::{PacketDispatcher, PacketSplitter, PacketWriter, U16BeLengthPrefixEncoder};
use base::net::state::{CHANNEL_BUFFER_SIZE, IoEventType, Protocol, Zip};
use base::tokio::sync::mpsc::Receiver;
use base::tokio_util::sync::CancellationToken;
use crossbeam_channel::TrySendError;
use parking_lot::Mutex;
use rtp_types::RtpPacket;
use socket2::Socket;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;
const RECV_BUF_SIZE: usize = 8 * 1024 * 1024;
const PARSE_RECOVERY_PACKET_COUNT: usize = 64;
pub fn listen_media_server(port: u16) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr =
        SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    net::listen(Protocol::ALL, socket_addr)
}

pub fn run(
    mut tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel: CancellationToken,
) -> GlobalResult<()> {
    let rtp_port = listener_port(&tu)?;
    if let Some(socket) = tu.1.take() {
        let socket2 = Socket::from(socket);

        socket2
            .set_recv_buffer_size(RECV_BUF_SIZE)
            .hand_log(|msg| error!("rtp io set recv_buffer failed: {msg}"))?;

        let actual_size = socket2
            .recv_buffer_size()
            .hand_log(|msg| error!("rtp io get recv_buffer failed: {msg}"))?;

        debug!(
            "rtp udp recv_buffer configured: requested={}, actual={}",
            RECV_BUF_SIZE, actual_size
        );

        tu.1 = Some(UdpSocket::from(socket2));
    }
    let (output_tx, output_rx) = base::tokio::sync::mpsc::channel(CHANNEL_BUFFER_SIZE);
    let writer: PacketWriter<U16BeLengthPrefixEncoder> =
        net::rw::direct_rw::<RtpReader, RtpReader, U16BeLengthPrefixEncoder>(
            tu,
            cancel.clone(),
            Arc::new(RtpReader::default()),
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
struct RtpReader {
    parse_failure_active: AtomicBool,
    parse_success_streak: AtomicUsize,
    parse_failure_episode: Mutex<FailureEpisode>,
}

impl RtpReader {
    fn forward_packet(
        &self,
        pkt: RtpPacket<'_>,
        payload: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        let ssrc = pkt.ssrc();
        let rtp_tx = match Register::refresh_rtp(ssrc, pkt.payload_type(), (remote_addr, protocol))
        {
            RefreshRtp::Ready(sender) => sender,
            RefreshRtp::UnknownSsrc => {
                Register::observe_unknown_rtp(ssrc, remote_addr, protocol);
                base::log::trace!("drop rtp packet for unknown ssrc; ssrc: {ssrc}");
                return Ok(());
            }
            RefreshRtp::Failed(error) => return Err(error),
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
                base::log::trace!("rtp input channel full; drop ssrc={ssrc}");
            }
            Err(TrySendError::Disconnected(_)) => {
                base::log::trace!("drop rtp packet for disconnected channel; ssrc: {ssrc}");
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
                self.record_parse_success();
                let payload_start = pkt.payload_offset();
                let payload_end = payload_start + pkt.payload_len();
                let payload = data.slice(payload_start..payload_end);
                self.forward_packet(pkt, payload, remote_addr, protocol)?;
            }
            Err(error) => {
                base::log::trace!("parse rtp packet failed: error={error}");
                self.record_parse_failure();
            }
        }
        Ok(())
    }
}

impl RtpReader {
    fn record_parse_failure(&self) {
        let mut episode = self.parse_failure_episode.lock();
        self.parse_success_streak.store(0, Ordering::Release);
        self.parse_failure_active.store(true, Ordering::Release);
        match episode.record_failure(Instant::now()) {
            EpisodeDecision::Started => warn!(
                "rtp input parse state changed: state=failed, previous_state=ready, reason=invalid_packet"
            ),
            EpisodeDecision::Summary {
                total,
                since_last_summary,
                suppressed,
                duration,
            } => warn!(
                "rtp input parse remains failed: state=failed, outcome=ongoing, reason=invalid_packet, total={total}, since_last_summary={since_last_summary}, suppressed={suppressed}, duration_ms={}",
                duration.as_millis()
            ),
            EpisodeDecision::Suppressed => {}
            EpisodeDecision::Recovered { .. } | EpisodeDecision::Healthy => unreachable!(),
        }
    }

    fn record_parse_success(&self) {
        if !self.parse_failure_active.load(Ordering::Acquire) {
            return;
        }
        let success_streak = self
            .parse_success_streak
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        if success_streak < PARSE_RECOVERY_PACKET_COUNT {
            return;
        }
        let mut episode = self.parse_failure_episode.lock();
        if self.parse_success_streak.load(Ordering::Acquire) < PARSE_RECOVERY_PACKET_COUNT {
            return;
        }
        if let EpisodeDecision::Recovered {
            total,
            suppressed,
            duration,
        } = episode.record_success(Instant::now())
        {
            info!(
                "rtp input parse state changed: state=ready, previous_state=failed, outcome=recovered, total_failures={total}, suppressed={suppressed}, duration_ms={}",
                duration.as_millis()
            );
        }
        self.parse_failure_active
            .store(episode.is_active(), Ordering::Release);
        self.parse_success_streak.store(0, Ordering::Release);
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
