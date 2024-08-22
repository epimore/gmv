use hyper::body;
use common::log::{info, warn};

use common::bytes::{BufMut, BytesMut};
use common::err::{GlobalError, GlobalResult, TransError};
use common::tokio::sync::broadcast;
use common::tokio::sync::broadcast::error::RecvError;

use crate::coder::h264::H264;
use crate::container::flv;
use crate::container::flv::{AvcDecoderConfigurationRecord, FlvHeader, PreviousTagSize, ScriptMetaData, TagHeader, TagType, VideoTagDataFirst};
use crate::general::mode::Coder;
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, mut rx: broadcast::Receiver<FrameData>) {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        let mut container = flv::MediaFlvContainer::register_all();
        loop {
            match rx.recv().await {
                Ok(FrameData { pay_type, timestamp, data }) => {
                    match pay_type {
                        Coder::PS => {}
                        Coder::MPEG4 => {}
                        Coder::H264(..) => {
                            if let Some((pkg, sps, pps, idr)) = container.flv_video_h264.packaging(data) {
                                let data = pkg.to_bytes();
                                let frame_data = FrameData {
                                    pay_type: Coder::H264(sps, pps, idr),
                                    timestamp,
                                    data,
                                };
                                if tx.send(frame_data).is_err() {
                                    info!("http 用户端已全部断开连接.");
                                    break;
                                }
                            }
                        }
                        Coder::SVAC_V => {}
                        Coder::H265 => {}
                        Coder::G711 => {}
                        Coder::SVAC_A => {}
                        Coder::G723_1 => {}
                        Coder::G729 => {}
                        Coder::G722_1 => {}
                        Coder::AAC => {}
                    }
                }
                Err(RecvError::Lagged(amt)) => {
                    warn!("ssrc={ssrc},数据包消费滞后{amt}条，Flv打包跳过.");
                }
                Err(RecvError::Closed) => {
                    info!("ssrc={ssrc},设备端流结束.");
                    break;
                }
            }
        }
    }
}

//当前仅支持h264，后面扩展时，需考虑flv script等内容，如添加audio等，是否将流信息放入cache
async fn first_frame(ssrc: u32, flv_tx: &mut body::Sender, rx: &mut broadcast::Receiver<FrameData>) -> GlobalResult<u32> {
    loop {
        match rx.recv().await {
            Ok(FrameData { pay_type, timestamp, data }) => {
                match pay_type {
                    Coder::PS => {}
                    Coder::MPEG4 => {}
                    Coder::H264(sps, pps, idr) => {
                        if let (Some(sps), Some(pps), true) = (sps, pps, idr) {
                            //flv header
                            let mut first_pkg = BytesMut::new();
                            let flv_header_bytes = FlvHeader::build(true, false).to_bytes();
                            first_pkg.put(flv_header_bytes);
                            let sps_nal = sps.slice(4..);
                            let pps_nal = pps.slice(4..);
                            //Script Tag
                            if let Ok((w, h, fr)) = H264::get_width_height_frame_rate(&sps_nal) {
                                let mut meta_data = ScriptMetaData::default();
                                meta_data.set_height(h as f64);
                                meta_data.set_width(w as f64);
                                // meta_data.set_videodatarate()
                                meta_data.set_videocodecid(7f64); //H.264视频编码的ID通常为 7
                                meta_data.set_framerate(fr);
                                if let Ok(meta_data_bytes) = meta_data.to_bytes() {
                                    let script_header_bytes = TagHeader::build(TagType::Script, 0, meta_data_bytes.len() as u32).to_bytes();
                                    let tag_size_bytes = PreviousTagSize::new((script_header_bytes.len() + meta_data_bytes.len()) as u32).previous_tag_size();
                                    first_pkg.put(script_header_bytes);
                                    first_pkg.put(meta_data_bytes);
                                    first_pkg.put(tag_size_bytes);
                                }
                            }
                            //Video Tag[0]
                            let con_record = AvcDecoderConfigurationRecord::build(sps_nal, pps_nal);
                            let data_tag0_bytes = VideoTagDataFirst::build(con_record).to_bytes();
                            let header_tag0_bytes = TagHeader::build(TagType::Video, 0, data_tag0_bytes.len() as u32).to_bytes();
                            let tag_size_bytes = PreviousTagSize::new((header_tag0_bytes.len() + data_tag0_bytes.len()) as u32).previous_tag_size();
                            first_pkg.put(header_tag0_bytes);
                            first_pkg.put(data_tag0_bytes);
                            first_pkg.put(tag_size_bytes);
                            //idr frame ->Video Tag[1]
                            let header_bytes = TagHeader::build(TagType::Video, 0, data.len() as u32).to_bytes();
                            let size_bytes = PreviousTagSize::new((header_bytes.len() + data.len()) as u32).previous_tag_size();
                            first_pkg.put(header_bytes);
                            first_pkg.put(data);
                            first_pkg.put(size_bytes);
                            flv_tx.send_data(first_pkg.freeze()).await.hand_log(|msg| warn!("{msg}"))?;
                            return Ok(timestamp);
                        }
                    }
                    Coder::SVAC_V => {}
                    Coder::H265 => {}
                    Coder::G711 => {}
                    Coder::SVAC_A => {}
                    Coder::G723_1 => {}
                    Coder::G729 => {}
                    Coder::G722_1 => {}
                    Coder::AAC => {}
                }
            }
            Err(RecvError::Lagged(amt)) => {
                warn!("ssrc={ssrc},first_frame:数据包消费滞后{amt}条，Flv打包跳过.");
            }
            Err(RecvError::Closed) => {
                return Err(GlobalError::new_biz_error(1199, "RecvError::Closed", |msg| info!("设备端流结束:{msg}")));
            }
        }
    }
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
pub async fn send_flv(ssrc: u32, mut flv_tx: body::Sender, mut rx: broadcast::Receiver<FrameData>) ->GlobalResult<()>{
    let start_time = first_frame(ssrc, &mut flv_tx, &mut rx).await?;
    loop {
        match rx.recv().await {
            Ok(FrameData { pay_type, timestamp, data }) => {
                match pay_type {
                    Coder::PS => {}
                    Coder::MPEG4 => {}
                    Coder::H264(..) => {
                        //a=rtpmap:96 H264/90000,->video_clock_rate=90000,单位毫秒 90000/1000 = 90
                        let header_bytes = TagHeader::build(TagType::Video, (timestamp - start_time) / 90, data.len() as u32).to_bytes();
                        let size_bytes = PreviousTagSize::new((header_bytes.len() + data.len()) as u32).previous_tag_size();
                        let mut bytes = BytesMut::with_capacity(header_bytes.len() + data.len() + size_bytes.len());
                        bytes.put(header_bytes);
                        bytes.put(data);
                        bytes.put(size_bytes);
                        flv_tx.send_data(bytes.freeze()).await.hand_log(|msg| info!("{msg}"))?;
                    }
                    Coder::SVAC_V => {}
                    Coder::H265 => {}
                    Coder::G711 => {}
                    Coder::SVAC_A => {}
                    Coder::G723_1 => {}
                    Coder::G729 => {}
                    Coder::G722_1 => {}
                    Coder::AAC => {}
                }
            }
            Err(RecvError::Lagged(amt)) => {
                warn!("ssrc={ssrc},跳过{amt}条数据");
            }
            Err(RecvError::Closed) => {
                return Err(GlobalError::new_biz_error(1199, "RecvError::Closed", |msg| info!("设备端流结束:{msg}")));
            }
        }
    }
}