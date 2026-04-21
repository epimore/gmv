use crate::io::splitter::rtp::TcpRtpBuffer;
use crate::state::register::Register;
use crate::{media, state};
use base::bytes::{Buf, Bytes, BytesMut};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error};
use base::log::{info, warn};
use base::net;
use base::net::reader::{PacketDispatcher, PacketSplitter};
use base::net::state::{Association, Package, Protocol, Zip};
use base::smallvec::SmallVec;
use base::tokio::select;
use base::tokio_util::sync::CancellationToken;
use crossbeam_channel::TrySendError;
use rtp_types::{RtpPacket, RtpParseError};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;

pub fn listen_media_server(port: u16) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr =
        SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"})?;
    let res = net::sdx::listen(Protocol::ALL, socket_addr);
    res
}

pub async fn run(
    tu: (Option<TcpListener>, Option<UdpSocket>),
    cancel: CancellationToken,
) -> GlobalResult<()> {
    net::reader::reader::<RtpReader, RtpReader>(tu, cancel, Arc::new(RtpReader::default()))
}
//
// pub async fn run(
//     tu: (Option<std::net::TcpListener>, Option<UdpSocket>),
//     cancel: CancellationToken,
// ) -> GlobalResult<()> {
//     let (output, mut input) = net::sdx::run_by_tokio(tu).await?;
//     let mut tcp_rtp_buffer = TcpRtpBuffer::register_buffer();
//     loop {
//         select! {
//             Some(zip) = input.recv() =>{
//                 match zip {
//                     Zip::Data(Package { association, data }) => {
//                         if association.protocol.eq(&Protocol::TCP) {
//                             let vec = tcp_rtp_buffer.fresh_data(association.local_addr, association.remote_addr, data);
//                             for rtp_data in vec {
//                                 demux_rtp(rtp_data, &association);
//                             }
//                         } else {
//                             demux_rtp(data, &association);
//                         }
//                     }
//                     Zip::Event(event) => {
//                         if event.type_code == 0 {
//                             tcp_rtp_buffer.remove_map(event.association.local_addr, event.association.remote_addr);
//                         }
//                     }
//                 }
//             }
//             _ = cancel.cancelled() => {
//                 let _ = output.send(Zip::build_shutdown_zip(None)).await;
//                 break;
//             }
//         }
//     }
//     Ok(())
// }
//
// fn demux_rtp(rtp_data: Bytes, association: &Association) {
//     match RtpPacket::parse(rtp_data.as_ref()) {
//         Ok(pkt) => {
//             let ssrc = pkt.ssrc();
//             match state::cache::refresh(ssrc, association, pkt.payload_type()) {
//                 None => {
//                     debug!("未知ssrc: {}", ssrc);
//                 }
//                 Some((rtp_tx, rtp_rx)) => {
//                     // let _ = util::dump("rtp_ps", &rtp_data, false);
//                     // let _ = util::dump("ps", pkt.payload(), false);
//                     let packet = media::rtp::RtpPacket {
//                         ssrc,
//                         timestamp: pkt.timestamp(),
//                         marker: pkt.marker_bit(),
//                         seq: pkt.sequence_number(),
//                         payload: Bytes::from(pkt.payload().to_vec()),
//                     };
//                     //通道满了，删除先入的数据
//                     if let Err(TrySendError::Full(_)) = rtp_tx.try_send(packet) {
//                         let _ = rtp_rx.recv().hand_log(|msg| info!("{msg}"));
//                         warn!("Err Full:丢弃ssrc={ssrc}");
//                     }
//                 }
//             }
//         }
//         Err(error) => {
//             warn!("parse rtp pkt error: {}", error.to_string());
//         }
//     }
// }

#[derive(Default)]
struct RtpReader;
impl PacketDispatcher for RtpReader {
    fn dispatch(
        &self,
        data: Bytes,
        remote_addr: SocketAddr,
        protocol: Protocol,
    ) -> GlobalResult<()> {
        match RtpPacket::parse(data.as_ref()) {
            Ok(pkt) => {
                let ssrc = pkt.ssrc();
                match Register::refresh_rtp(ssrc, pkt.payload_type(), (remote_addr, protocol)) {
                    None => {
                        return Err(GlobalError::new_biz_error(
                            BaseErrorCode::NotFound.code(),
                            "rtp channel has closed",
                            |msg| error!("{msg}"),
                        ));
                    }
                    Some(rtp_tx) => {
                        let packet = media::rtp::RtpPacket {
                            ssrc,
                            timestamp: pkt.timestamp(),
                            marker: pkt.marker_bit(),
                            seq: pkt.sequence_number(),
                            payload: Bytes::copy_from_slice(pkt.payload()),
                        };
                        //通道满了，删除先入的数据
                        match rtp_tx.try_send(packet) {
                            Ok(_) => {}
                            Err(TrySendError::Full(_)) => {
                                warn!("Err Full:丢弃ssrc={ssrc}");
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                return Err(GlobalError::new_biz_error(
                                    BaseErrorCode::NotFound.code(),
                                    "rtp channel has closed",
                                    |msg| error!("{msg}"),
                                ));
                            }
                        }
                    }
                }
            }
            Err(error) => {
                warn!("parse rtp pkt error: {}", error);
            }
        }
        Ok(())
    }
}
const TCP_RTP_HEADER_LEN: usize = 2;
const MIN_RTP_HEADER_LEN: usize = 12;
//tcp封装的Rtp包：2 bytes Data_len + N bytes Rtp_data(MIN_RTP_HEADER_LEN = 12)
const TCP_DATA_BASE_LEN: usize = TCP_RTP_HEADER_LEN + MIN_RTP_HEADER_LEN;
const MAX_LIMIT_RTP_PACKET_SIZE: usize = 1024 * 16;
impl PacketSplitter for RtpReader {
    fn feed<F>(&mut self, buffer: &mut BytesMut, mut f: F) -> GlobalResult<()>
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
                    |msg| error!("{msg}: max = {}, this = {}", MAX_LIMIT_RTP_PACKET_SIZE, len),
                ));
            }

            let split_len = len + TCP_RTP_HEADER_LEN;

            if buffer.len() < split_len {
                break;
            }
            let mut split_data = buffer.split_to(split_len);
            let rtp_data = split_data.split_off(TCP_RTP_HEADER_LEN).freeze();
            f(rtp_data)?
        }
        Ok(())
    }
}
