use std::net::SocketAddr;
use std::str::FromStr;
use bytes::Bytes;

use rtp::packet::Packet;
use webrtc_util::marshal::Unmarshal;

use common::exception::{TransError};
use common::log::{info, warn};
use common::log::{debug, error};
use common::net;
use common::net::state::{Association, Package, Protocol, Zip};

use crate::container::rtp::TcpRtpBuffer;
use crate::state;

pub async fn run(port: u16) {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"}).expect("监听地址无效");
    let (_output, mut input) = net::init_net(Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
    let mut tcp_rtp_buffer = TcpRtpBuffer::register_buffer();
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package { association, data }) => {
                if association.protocol.eq(&Protocol::TCP) {
                    let vec = tcp_rtp_buffer.fresh_data(association.local_addr, association.remote_addr, data);
                    for rtp_data in vec {
                        demux_rtp(rtp_data, &association).await;
                    }
                } else {
                    demux_rtp(data, &association).await;
                }
            }
            Zip::Event(event) => {
                if event.type_code == 0 {
                    tcp_rtp_buffer.remove_map(event.association.local_addr, event.association.remote_addr);
                }
            }
        }
    }
}

async fn demux_rtp(mut rtp_data: Bytes, association: &Association) {
    match Packet::unmarshal(&mut rtp_data) {
        Ok(pkt) => {
            let ssrc = pkt.header.ssrc;
            match state::cache::refresh(ssrc, association).await {
                None => {
                    debug!("未知ssrc: {}",ssrc);
                }
                Some((rtp_tx, rtp_rx)) => {
                    //通道满了，删除先入的数据
                    if let Err(async_channel::TrySendError::Full(_)) = rtp_tx.try_send(pkt) {
                        let d_pkg = rtp_rx.recv().await.hand_log(|msg| info!("{msg}"));
                        warn!("Err Full:丢弃ssrc={ssrc}.seq={}",d_pkg.unwrap().header.sequence_number);
                    }
                }
            }
        }
        Err(error) => {
            warn!("unmarshal rtp pkt error: {}",error.to_string());
        }
    }
}
