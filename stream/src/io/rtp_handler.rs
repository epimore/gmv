use base::bytes::Bytes;
use crossbeam_channel::TrySendError;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;

use crate::io::splitter::rtp::TcpRtpBuffer;
use crate::{media, state};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{debug, error};
use base::log::{info, warn};
use base::net;
use base::net::state::{Association, Package, Protocol, Zip};
use base::tokio::select;
use base::tokio_util::sync::CancellationToken;
use rtp_types::RtpPacket;

pub fn listen_media_server(port: u16) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr =
        SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"})?;
    let res = net::sdx::listen(Protocol::ALL, socket_addr);
    res
}

pub async fn run(
    tu: (Option<std::net::TcpListener>, Option<UdpSocket>),
    cancel: CancellationToken,
) -> GlobalResult<()> {
    let (output, mut input) = net::sdx::run_by_tokio(tu).await?;
    let mut tcp_rtp_buffer = TcpRtpBuffer::register_buffer();
    loop {
        select! {
            Some(zip) = input.recv() =>{
                match zip {
                    Zip::Data(Package { association, data }) => {
                        if association.protocol.eq(&Protocol::TCP) {
                            let vec = tcp_rtp_buffer.fresh_data(association.local_addr, association.remote_addr, data);
                            for rtp_data in vec {
                                demux_rtp(rtp_data, &association);
                            }
                        } else {
                            demux_rtp(data, &association);
                        }
                    }
                    Zip::Event(event) => {
                        if event.type_code == 0 {
                            tcp_rtp_buffer.remove_map(event.association.local_addr, event.association.remote_addr);
                        }
                    }
                }
            }
            _ = cancel.cancelled() => {
                let _ = output.send(Zip::build_shutdown_zip(None)).await;
                break;
            }
        }
    }
    Ok(())
}

fn demux_rtp(rtp_data: Bytes, association: &Association) {
    match RtpPacket::parse(rtp_data.as_ref()) {
        Ok(pkt) => {
            let ssrc = pkt.ssrc();
            match state::cache::refresh(ssrc, association, pkt.payload_type()) {
                None => {
                    debug!("未知ssrc: {}", ssrc);
                }
                Some((rtp_tx, rtp_rx)) => {
                    // let _ = util::dump("rtp_ps", &rtp_data, false);
                    // let _ = util::dump("ps", pkt.payload(), false);

                    let packet = media::rtp::RtpPacket {
                        ssrc,
                        timestamp: pkt.timestamp(),
                        marker: pkt.marker_bit(),
                        seq: pkt.sequence_number(),
                        payload: Bytes::from(pkt.payload().to_vec()),
                    };
                    //通道满了，删除先入的数据
                    if let Err(TrySendError::Full(_)) = rtp_tx.try_send(packet) {
                        let _ = rtp_rx.recv().hand_log(|msg| info!("{msg}"));
                        warn!("Err Full:丢弃ssrc={ssrc}");
                    }
                }
            }
        }
        Err(error) => {
            warn!("parse rtp pkt error: {}", error.to_string());
        }
    }
}
