use std::net::SocketAddr;
use std::str::FromStr;

use crossbeam_channel::{bounded, Sender, TrySendError};
use discortp::demux;
use discortp::demux::Demuxed;
use discortp::rtp::RtpType;
use xrtsp::rtp::RtpPacket;
use xrtsp::rtp::utils::Unmarshal;

use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::log::{debug, error, info};
use common::log::Level::Debug;
use common::net;
use common::net::shared::Zip;

use crate::general::mode::ServerConf;
use crate::state;

pub async fn run(port: u16) {
    let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", port)).hand_err(|msg| error! {"{msg}"}).expect("监听地址无效");
    let (output, mut input) = net::init_net(net::shared::Protocol::ALL, socket_addr).await.hand_err(|msg| error!("{msg}")).expect("网络监听失败");
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(data) => {
                //todo 自己解析RTP...
                match demux::demux(data.get_data()) {
                    Demuxed::Rtp(rtp_packet) => {
                        //todo ssrc
                        match state::cache::refresh(1, data.get_bill()).await {
                            None => {
                                debug!("未知ssrc: {}",rtp_packet.get_ssrc())
                            }
                            Some((rtp_tx, rtp_rx)) => {
                                if let RtpType::Dynamic(v) = rtp_packet.get_payload_type() {
                                    if v <= 100 {
                                        //通道满了，删除先入的数据
                                        if let Err(TrySendError::Full(_)) = rtp_tx.try_send(data.get_owned_data()) {
                                            let _ = rtp_rx.recv().hand_err(|msg| debug!("{msg}"));
                                        }
                                    }
                                } else {
                                    info!("暂不支持数据类型: tp = {:?}",rtp_packet.get_payload_type())
                                }
                            }
                        }
                    }
                    Demuxed::Rtcp(_) => {}
                    Demuxed::FailedParse(_) => {}
                    Demuxed::TooSmall => {}
                }
            }
            Zip::Event(event) => {
                //TCP连接断开，告知信令端
            }
        }
    }
}
