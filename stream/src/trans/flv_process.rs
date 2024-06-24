use byteorder::BigEndian;
use hyper::body;
use log::warn;
use xflv::define::tag_type;
use xflv::muxer::{FlvMuxer, HEADER_LENGTH};

use common::anyhow::anyhow;
use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::broadcast;
use common::tokio::sync::broadcast::{Receiver, Sender};

use crate::container::flv::{AVCDecoderConfiguration, FlvHeader, FlvTag, TagType};
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, mut rx: crossbeam_channel::Receiver<FrameData>) -> GlobalResult<()> {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        while let Ok(frameData) = rx.recv() {
            let sender = tx.clone();
            let handle_muxer_data_fn = Box::new(
                move |data: Bytes| -> GlobalResult<()> {
                    // println!("flv sender channel len = {}", sender.len());
                    if let Err(err) = sender.send(data) {
                        log::error!("send flv tag error: {}", err);
                    }
                    Ok(())
                },
            );
            match frameData {
                FrameData::Video { timestamp, data } => {
                    FlvTag::process(handle_muxer_data_fn, TagType::Video, timestamp, data)?
                }
                FrameData::Audio { timestamp, data } => { println!("ts = {timestamp}, data len = {}", data.len()); }
                FrameData::MetaData { timestamp, data } => { println!("ts = {timestamp}, data len = {}", data.len()); }
            }
        }
    }
    Ok(())
}


//h264
//首帧为IDR帧，实现画面秒开
pub async fn send_flv(mut flv_tx: body::Sender, mut rx: Receiver<Bytes>) {
    //sps
    let mut sps_naul: Option<Bytes> = None;
    //pps
    let mut pps_naul: Option<Bytes> = None;
    while let Ok(bytes) = rx.recv().await {
        println!("flv receiver channel len = {}", rx.len());
        // Tag Header = TagType+DataSize+TimeStamp+TimestampExtended+StreamId = 11 byte
        // FrameType+CodecID+CodecID+CompositionTime = 5 byte
        if bytes.len() > 11 {
            //bytes[0]->video, bytes[12]->h264
            if bytes[0] == 9 && bytes[12] == 0x17 {
                //bytes[17]->h264 nalu type
                match bytes[17] & 0x1F {
                    //强制IDR帧
                    5 => {
                        if let (Some(sps), Some(pps)) = (&sps_naul, &pps_naul) {
                            println!("flv receiver1 channel len = {}", rx.len());
                            //FLV HEADER
                            let (hdr, tag_size_0) = FlvHeader::get_header_byte_and_previos_tag_size0(true, true);
                            let _ = flv_tx.send_data(hdr).await.hand_log_err();
                            let _ = flv_tx.send_data(tag_size_0).await.hand_log_err();
                            //FLV BODY
                            //todo flv script tag
                            //sps pps
                            let configuration_bytes = AVCDecoderConfiguration::new(sps.slice(..), pps.slice(..), 0).to_flv_tag_bytes();
                            let len = configuration_bytes.len() as u32;
                            let _ = flv_tx.send_data(configuration_bytes).await.hand_log_err();
                            let _ = flv_tx.send_data(Bytes::from(len.to_be_bytes().to_vec())).await.hand_log_err();
                            //idr
                            let _ = flv_tx.send_data(bytes).await.hand_log_err();
                            break;
                        }
                    }
                    7 => { sps_naul = Some(bytes); }
                    8 => { pps_naul = Some(bytes); }
                    _ => {}
                }
            }
        }
    }

    loop {
        match rx.recv().await {
            Ok(bytes) => {
                println!("flv receiver2 channel len = {}", rx.len());
                let _ = flv_tx.send_data(bytes).await.hand_log_err();
            }
            Err(broadcast::error::RecvError::Lagged(amt)) => {
                rx = rx.resubscribe();
            }
            Err(..) => {}
        }
    }
}