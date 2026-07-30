use crate::media;
use crate::state::register::{RefreshRtp, Register};
use base::bytes::{Bytes, BytesMut};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error, info, warn};
use base::logger::episode::{EpisodeDecision, FailureEpisode};
use base::net;
use base::net::rw::{ManagedPacketIo, PacketDispatcher, PacketSplitter, U16BeLengthPrefixEncoder};
use base::net::state::Protocol;
use base::tokio_util::sync::CancellationToken;
use base::utils::rt::GlobalRuntime;
use crossbeam_channel::TrySendError;
use parking_lot::Mutex;
use rtp_types::RtpPacket;
use socket2::Socket;
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;
const RECV_BUF_SIZE: usize = 8 * 1024 * 1024;
const PARSE_RECOVERY_PACKET_COUNT: usize = 64;
pub fn listen_media_server(
    bind_ip: IpAddr,
    port: u16,
) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr = SocketAddr::new(bind_ip, port);
    net::listen(Protocol::ALL, socket_addr)
}

pub fn start_managed(
    runtime: &GlobalRuntime,
    task_name: impl Into<String>,
    mut tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel: CancellationToken,
    dispatch: Arc<EndpointDispatchContext>,
) -> GlobalResult<ManagedPacketIo<U16BeLengthPrefixEncoder>> {
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
    net::rw::managed_direct_rw::<RtpReader, RtpPacketSplitter, U16BeLengthPrefixEncoder>(
        runtime,
        task_name,
        tu,
        cancel,
        Arc::new(RtpReader::new(dispatch)),
        Arc::new(U16BeLengthPrefixEncoder),
    )
}

pub struct EndpointDispatchContext {
    pub endpoint_id: String,
    pub generation: u64,
    pub stream_id: Option<String>,
    pub expected_ssrc: Option<u32>,
    exclusive_peer: bool,
    active: AtomicBool,
    observed: AtomicBool,
    tcp_peer: Mutex<Option<SocketAddr>>,
    udp_peer: Mutex<Option<SocketAddr>>,
}

impl EndpointDispatchContext {
    pub fn new(
        endpoint_id: String,
        generation: u64,
        stream_id: Option<String>,
        expected_ssrc: Option<u32>,
        exclusive_peer: bool,
    ) -> Self {
        Self {
            endpoint_id,
            generation,
            stream_id,
            expected_ssrc,
            exclusive_peer,
            active: AtomicBool::new(true),
            observed: AtomicBool::new(false),
            tcp_peer: Mutex::new(None),
            udp_peer: Mutex::new(None),
        }
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn is_observed(&self) -> bool {
        self.observed.load(Ordering::Acquire)
    }

    fn accepts_peer(&self, remote_addr: SocketAddr, protocol: Protocol) -> bool {
        if !self.exclusive_peer {
            return true;
        }
        let peer = match protocol {
            Protocol::TCP => &self.tcp_peer,
            Protocol::UDP => &self.udp_peer,
            Protocol::ALL => return false,
        };
        let mut peer = peer.lock();
        match *peer {
            Some(current) => current == remote_addr,
            None => {
                *peer = Some(remote_addr);
                true
            }
        }
    }

    fn close_peer(&self, remote_addr: SocketAddr, protocol: Protocol) {
        if protocol != Protocol::TCP || !self.exclusive_peer {
            return;
        }
        let mut peer = self.tcp_peer.lock();
        if peer.is_some_and(|current| current == remote_addr) {
            *peer = None;
        }
    }
}

struct RtpReader {
    dispatch: Arc<EndpointDispatchContext>,
    parse_failure_active: AtomicBool,
    parse_success_streak: AtomicUsize,
    parse_failure_episode: Mutex<FailureEpisode>,
}

impl RtpReader {
    fn new(dispatch: Arc<EndpointDispatchContext>) -> Self {
        Self {
            dispatch,
            parse_failure_active: AtomicBool::new(false),
            parse_success_streak: AtomicUsize::new(0),
            parse_failure_episode: Mutex::new(FailureEpisode::default()),
        }
    }

    fn forward_packet(
        &self,
        pkt: RtpPacket<'_>,
        payload: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        let ssrc = pkt.ssrc();
        if !self.dispatch.active.load(Ordering::Acquire) {
            return Ok(());
        }
        if self
            .dispatch
            .expected_ssrc
            .is_some_and(|expected| expected != ssrc)
        {
            base::log::trace!(
                "drop rtp packet for endpoint SSRC mismatch: endpoint_id={}, generation={}, expected_ssrc={:?}, actual_ssrc={ssrc}",
                self.dispatch.endpoint_id,
                self.dispatch.generation,
                self.dispatch.expected_ssrc
            );
            return Ok(());
        }
        if let Some(stream_id) = self.dispatch.stream_id.as_deref()
            && Register::stream_id_by_ssrc(ssrc).is_some_and(|actual| actual.as_ref() != stream_id)
        {
            base::log::trace!(
                "drop rtp packet for endpoint stream mismatch: endpoint_id={}, generation={}, stream_id={stream_id}, ssrc={ssrc}",
                self.dispatch.endpoint_id,
                self.dispatch.generation
            );
            return Ok(());
        }
        if let Some(stream_id) = self.dispatch.stream_id.as_deref()
            && !Register::media_endpoint_matches(
                stream_id,
                &self.dispatch.endpoint_id,
                self.dispatch.generation,
                ssrc,
            )
        {
            base::log::trace!(
                "drop rtp packet for stale endpoint generation: endpoint_id={}, generation={}, stream_id={stream_id}, ssrc={ssrc}",
                self.dispatch.endpoint_id,
                self.dispatch.generation
            );
            return Ok(());
        }
        if !self.dispatch.accepts_peer(remote_addr, protocol) {
            base::log::trace!(
                "drop rtp packet from unexpected association: endpoint_id={}, generation={}, protocol={protocol}, remote_addr={remote_addr}",
                self.dispatch.endpoint_id,
                self.dispatch.generation
            );
            return Ok(());
        }
        self.dispatch.observed.store(true, Ordering::Release);
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

    fn close(&self, remote_addr: SocketAddr, protocol: Protocol) -> GlobalResult<()> {
        self.dispatch.close_peer(remote_addr, protocol);
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

#[derive(Default)]
struct RtpPacketSplitter;

impl PacketSplitter for RtpPacketSplitter {
    fn feed_owned<F>(&mut self, buffer: &mut BytesMut, f: F) -> GlobalResult<()>
    where
        F: FnMut(Bytes) -> GlobalResult<()>,
    {
        feed_tcp_packets(buffer, f)
    }
}

#[cfg(test)]
mod tests {
    use super::EndpointDispatchContext;
    use base::net::state::Protocol;
    use std::net::SocketAddr;

    #[test]
    fn dedicated_endpoint_pins_one_peer_per_transport() {
        let dispatch = EndpointDispatchContext::new(
            "endpoint-1".to_string(),
            1,
            Some("stream-1".to_string()),
            Some(1001),
            true,
        );
        let first: SocketAddr = "127.0.0.1:30001".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:30002".parse().unwrap();

        assert!(dispatch.accepts_peer(first, Protocol::TCP));
        assert!(dispatch.accepts_peer(first, Protocol::TCP));
        assert!(!dispatch.accepts_peer(second, Protocol::TCP));
        assert!(dispatch.accepts_peer(first, Protocol::UDP));
        assert!(!dispatch.accepts_peer(second, Protocol::UDP));
        dispatch.close_peer(first, Protocol::TCP);
        assert!(dispatch.accepts_peer(second, Protocol::TCP));
    }

    #[test]
    fn shared_endpoint_accepts_multiple_tcp_associations() {
        let dispatch = EndpointDispatchContext::new("single".to_string(), 1, None, None, false);
        let first: SocketAddr = "127.0.0.1:30001".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:30002".parse().unwrap();

        assert!(dispatch.accepts_peer(first, Protocol::TCP));
        assert!(dispatch.accepts_peer(second, Protocol::TCP));
        assert!(dispatch.accepts_peer(first, Protocol::UDP));
        assert!(dispatch.accepts_peer(second, Protocol::UDP));
    }
}
