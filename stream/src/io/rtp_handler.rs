use std::net::SocketAddr;
use std::str::FromStr;

use rtp::packet::Packet;
use webrtc_util::marshal::Unmarshal;

use common::err::TransError;
use common::log::{info, warn};
use common::log::{debug, error};
use common::net;
use common::net::shared::{Package, Protocol, Zip};

use crate::container::rtp::TcpRtpBuffer;
use crate::state;

pub async fn run(port: u16) {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"}).expect("监听地址无效");
    let (_output, mut input) = net::init_net(Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
    let mut tcp_rtp_buffer = TcpRtpBuffer::register_buffer();
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package { bill, mut data }) => {
                if bill.protocol.eq(&Protocol::TCP) {
                    match tcp_rtp_buffer.fresh_data(bill.local_addr, bill.remote_addr, data) {
                        None => {
                            continue;
                        }
                        Some(rtp_data) => {
                            data = rtp_data;
                        }
                    }
                }
                match Packet::unmarshal(&mut data) {
                    Ok(pkt) => {
                        let ssrc = pkt.header.ssrc;
                        let pt = pkt.header.payload_type;
                        if matches!(pt,96|98|100|102) {
                            //todo 将pt放入，第一次回调时，检查pt是否符合，不符合以事件的形式发送给信令
                            match state::cache::refresh(ssrc, &bill).await {
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
                        } else {
                            //todo 回调通知调用方
                            warn!("ssrc = {}; 暂不支持数据类型: {:?}",ssrc,pt);
                        }
                    }
                    Err(error) => {
                        warn!("unmarshal rtp pkt error: {}",error.to_string());
                    }
                }
            }
            Zip::Event(event) => {
                //todo TCP连接断开，告知信令端
                if event.type_code == 0 {
                    tcp_rtp_buffer.remove_map(event.bill.local_addr, event.bill.remote_addr);
                }
            }
        }
    }
}
