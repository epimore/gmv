use log::{debug, error, info, warn};
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp::packetizer::Depacketizer;

use common::anyhow::anyhow;
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::mpsc::UnboundedSender;

use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, tx: crossbeam_channel::Sender<FrameData>) -> GlobalResult<()> {
    if let Some(rx) = cache::get_rtp_rx(&ssrc) {
        let mut h264packet = H264Packet::default();
        loop {
            match rx.recv() {
                Ok(pkt) => {
                    match pkt.header.payload_type {
                        98 => {}
                        96 => {
                            match h264packet.depacketize(&pkt.payload) {
                                Ok(byte) => {
                                    if byte.len()>0 {
                                        let frame_data = FrameData::Video { timestamp: pkt.header.timestamp, data: byte };
                                        let _ = tx.send(frame_data).hand_log_err();
                                    }
                                }
                                Err(error) => {
                                    println!("h264packet depacketize err = {:?}",error);
                                }
                            }
                        }
                        100 => {}
                        102 => {}
                        _ => {
                            return Err(GlobalError::new_biz_error(4005, "系统暂不支持", |msg| debug!("{msg}")));
                        }
                    }
                }
                Err(_) => {
                    info!("ssrc = {ssrc},流已释放");
                    break;
                }
            }
        }
    }
    Ok(())
}
