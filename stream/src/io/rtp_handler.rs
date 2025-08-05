use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use common::bytes::Bytes;
use crossbeam_channel::TrySendError;

use common::exception::{GlobalResult, GlobalResultExt};
use common::log::{info, warn};
use common::log::{debug, error};
use common::net;
use common::net::state::{Association, Package, Protocol, Zip};
use rtp_types::RtpPacket;
use crate::{media, state};
use crate::io::splitter::rtp::TcpRtpBuffer;

pub fn listen_gb_server(port: u16) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"})?;
    let res = net::sdx::listen(net::state::Protocol::ALL, socket_addr);
    info!("Listen to rtp over tcp and udp,stream addr = 0.0.0.0:{port}...");
    res
}


pub async fn run(tu: (Option<std::net::TcpListener>, Option<UdpSocket>)) -> GlobalResult<()> {
    let (_output, mut input) = net::sdx::run_by_tokio(tu).await?;
    let mut tcp_rtp_buffer = TcpRtpBuffer::register_buffer();
    while let Some(zip) = input.recv().await {
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
    error!("流媒体服务异常退出");
    Ok(())
}

fn demux_rtp(rtp_data: Bytes, association: &Association) {
    match  RtpPacket::parse(rtp_data.as_ref()){
        Ok(pkt) => {
            let ssrc = pkt.ssrc();
            match state::cache::refresh(ssrc, association, pkt.payload_type()) {
                None => {
                    debug!("未知ssrc: {}",ssrc);
                }
                Some((rtp_tx, rtp_rx)) => {
                    let packet = media::rtp::RtpPacket {
                        ssrc,
                        seq: pkt.sequence_number(),
                        data: rtp_data,
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
            warn!("parse rtp pkt error: {}",error.to_string());
        }
    }
}
