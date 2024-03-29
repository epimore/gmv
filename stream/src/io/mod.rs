use std::net::SocketAddr;
use std::str::FromStr;
use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::log::error;
use common::net;
use common::net::shard::Zip;
use crate::general::mode::Stream;
use discortp::demux;
use discortp::demux::Demuxed;
use discortp::pnet::packet::{PacketData, PrimitiveValues};
use discortp::rtcp::RtcpPacket;
use discortp::rtp::{RtpPacket, RtpType};


pub trait IO {
    async fn listen_input(&self);
}

impl IO for Stream {
    async fn listen_input(&self) {
        let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", self.get_port())).hand_err(|msg| error! {"{msg}"}).expect("监听地址无效");
        let (output, mut input) = net::init_net(net::shard::Protocol::ALL, socket_addr).await.hand_err(|msg| error!("{msg}")).expect("网络监听失败");
        while let Some(zip) = input.recv().await {
            match zip {
                Zip::Data(data) => {
                    match demux::demux(data.get_data()) {
                        Demuxed::Rtp(rtp_packet) => {}
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
}

async fn do_cache(rtp_packet: RtpPacket<'_>) -> GlobalResult<()> {
    let ssrc = rtp_packet.get_ssrc() as u32;
    let sn = rtp_packet.get_sequence().0.0;
    let ts = rtp_packet.get_timestamp().0.0;


    unimplemented!()
}