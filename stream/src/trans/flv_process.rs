use byteorder::BigEndian;
use hyper::body;
use log::warn;

use common::anyhow::anyhow;
use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::broadcast;
use common::tokio::sync::broadcast::{Receiver, Sender};
use crate::coder::h264::H264SPS;

use crate::container::flv::{AVCDecoderConfiguration, FlvHeader, FlvTag, ScriptTag, TagType};
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, mut rx: crossbeam_channel::Receiver<FrameData>) -> GlobalResult<()> {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        while let Ok(frameData) = rx.recv() {
            let sender = tx.clone();
            let handle_muxer_data_fn = Box::new(
                move |data: Bytes| -> GlobalResult<()> {
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
//         0x67 (0 11 00111) SPS    非常重要       type = 7
//         0x68 (0 11 01000) PPS     非常重要       type = 8
//         0x65 (0 11 00101) IDR帧  关键帧  非常重要 type = 5
//         0x61 (0 11 00001) I帧        重要         type=1    非IDR的I帧 不大常见
//         0x41 (0 10 00001) P帧      重要         type = 1
//         0x01 (0 00 00001) B帧     不重要        type = 1
//         0x06 (0 00 00110) SEI     不重要        type = 6
//首帧为IDR帧，实现画面秒开
pub async fn send_flv(mut flv_tx: body::Sender, mut rx: Receiver<Bytes>) {
    //sps
    let mut sps_naul: Option<Bytes> = None;
    //pps
    let mut pps_naul: Option<Bytes> = None;
    while let Ok(bytes) = rx.recv().await {
        // Tag Header = TagType+DataSize+TimeStamp+TimestampExtended+StreamId = 11 byte
        // FrameType+CodecID+CodecID+CompositionTime = 5 byte
        if bytes.len() > 11 {
            //bytes[0]->video, bytes[12]->h264
            if bytes[0] == 9 && bytes[11] == 0x17 {
                // println!("flv receiver channel {:02x?}", &bytes[..17].to_vec());
                //bytes[16]->h264 nalu type
                match bytes[16] & 0x1F {
                    //强制IDR帧
                    5 => {
                        if let (Some(sps), Some(pps)) = (&sps_naul, &pps_naul) {
                            //FLV HEADER
                            let (hdr, tag_size_0) = FlvHeader::get_header_byte_and_previos_tag_size0(true, false);
                            let _ = flv_tx.send_data(hdr).await.hand_log(|msg| warn!("{msg}"));
                            let _ = flv_tx.send_data(tag_size_0).await.hand_log(|msg| warn!("{msg}"));
                            //FLV BODY
                            // flv script tag
                            if let Ok(Some(h264sps)) = H264SPS::get_sps_info_by_nalu(0, sps) {
                                let ((c, w, h, r)) = h264sps.get_c_w_h_r();
                                println!("---- {c},{w},{h},{r}");
                                if let Ok(script_tag_data) = ScriptTag::build_script_tag_data(w, h, r) {
                                    let script_tag_bytes = ScriptTag::build_script_tag_bytes(script_tag_data);
                                    let _ = flv_tx.send_data(script_tag_bytes).await.hand_log(|msg| warn!("{msg}"));
                                }
                            }

                            //sps pps
                            // let configuration_bytes = AVCDecoderConfiguration::new(sps.slice(..), pps.slice(..), 0).to_flv_tag_bytes();
                            // let len = configuration_bytes.len() as u32;
                            // let _ = flv_tx.send_data(configuration_bytes).await.hand_log(|msg| warn!("{msg}"));
                            // let _ = flv_tx.send_data(Bytes::from(len.to_be_bytes().to_vec())).await.hand_log(|msg| warn!("{msg}"));
                            // //idr
                            // let _ = flv_tx.send_data(bytes).await.hand_log(|msg| warn!("{msg}"));
                            break;
                        }
                    }
                    7 => {
                        // println!("sps = {:02x?}", bytes.to_vec());
                        println!("sps len = {}", bytes.len());
                        sps_naul = Some(Bytes::from(bytes[16..].to_vec()));
                    }
                    8 => {
                        // println!("pps = {:02x?}", bytes.to_vec());
                        println!("pps len = {}", bytes.len());
                        pps_naul = Some(Bytes::from(bytes[16..].to_vec()));
                    }
                    _ => {}
                }
            }
        }
    }
    panic!();
    loop {
        match rx.recv().await {
            Ok(bytes) => {
                let _ = flv_tx.send_data(bytes).await.hand_log(|msg| warn!("{msg}"));
            }
            Err(broadcast::error::RecvError::Lagged(amt)) => {
                rx = rx.resubscribe();
            }
            Err(..) => {}
        }
    }
}

