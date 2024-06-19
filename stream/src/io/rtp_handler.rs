use std::net::SocketAddr;
use std::str::FromStr;

use crossbeam_channel::TrySendError;
use log::warn;
use webrtc::rtp::packet::Packet;
use webrtc::util::{Error, Unmarshal};

use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::log::{debug, error, info};
use common::log::Level::Debug;
use common::net;
use common::net::shared::{Package, Zip};

use crate::general::mode::ServerConf;
use crate::state;

pub async fn run(port: u16) {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_log(|msg| error! {"{msg}"}).expect("监听地址无效");
    let (output, mut input) = net::init_net(net::shared::Protocol::ALL, socket_addr).await.hand_log(|msg| error!("{msg}")).expect("网络监听失败");
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(Package{ bill, mut data }) => {
                //todo RTP包不完整?
                match Packet::unmarshal(&mut data) {
                    Ok(pkt) => {
                        //todo ssrc:pkt.header.ssrc
                        let ssrc = pkt.header.ssrc;
                        match state::cache::refresh(1, &bill).await {
                            None => {
                                debug!("未知ssrc: {}",ssrc);
                            }
                            Some((rtp_tx, rtp_rx)) => {
                                let pt = pkt.header.payload_type;
                                if pt <= 100 {
                                    //通道满了，删除先入的数据
                                    if let Err(TrySendError::Full(_)) = rtp_tx.try_send(pkt) {
                                        let _ = rtp_rx.recv().hand_log(|msg| debug!("{msg}"));
                                    }
                                } else {
                                    warn!("暂不支持数据类型: {:?}",pt)
                                }
                            }
                        }
                    }
                    Err(error) => {
                        warn!("unmarshal rtp pkt error: {}",error.to_string());
                    }
                }
            }
            Zip::Event(event) => {
                //TCP连接断开，告知信令端
            }
        }
    }
}
