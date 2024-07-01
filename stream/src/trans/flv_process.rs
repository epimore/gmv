use hyper::body;
use log::warn;

use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::tokio::sync::broadcast;

use crate::coder::h264::H264;
use crate::container::flv;
use crate::container::flv::{AvcDecoderConfigurationRecord, FlvHeader, PreviousTagSize, ScriptMetaData, TagHeader, TagType, VideoTagDataFirst};
use crate::general::mode::Coder;
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, mut rx: broadcast::Receiver<FrameData>) -> GlobalResult<()> {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        let mut container = flv::MediaFlvContainer::register_all();
        while let Ok(FrameData { pay_type, timestamp, data }) = rx.recv().await {
            match pay_type {
                Coder::PS => {}
                Coder::MPEG4 => {}
                Coder::H264 => {
                    if let Some(pkg) = container.flv_video_h264.packaging(data) {
                        let data_bytes = pkg.to_bytes();
                        let header_bytes = TagHeader::build(TagType::Video, timestamp, data_bytes.len() as u32).to_bytes();
                        let size_bytes = PreviousTagSize::new((header_bytes.len() + data_bytes.len()) as u32).previous_tag_size();
                        let mut tag = BytesMut::with_capacity(header_bytes.len() + data_bytes.len() + size_bytes.len());
                        tag.put(header_bytes);
                        tag.put(data_bytes);
                        tag.put(size_bytes);
                        let _ = tx.send(tag.freeze()).hand_log(|msg| warn!("{msg}"));
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
    }
    Ok(())
}

//当前仅支持h264，后面扩展时，需考虑flv script等内容，如添加audio等，是否将流信息放入cache
async fn first_frame(flv_tx: &mut body::Sender, rx: &mut broadcast::Receiver<Bytes>) {
    while let Ok(bytes) = rx.recv().await {
        //获取带有sps信息的数据包:头信息tag_header(11)+frame_type_codec_id(1)+avc_packet_type(1)+composition_time_offset(3)+nal_size(4) = 20;
        if bytes[20] & 0x1f == 7 {
            let mut first_pkg = BytesMut::new();
            let flv_header_bytes = FlvHeader::build(true, false).to_bytes();
            first_pkg.put(flv_header_bytes);
            let ts = u32::from_be_bytes([bytes[7], bytes[4], bytes[5], bytes[6]]);
            let sps_size = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
            let sps_nal = bytes.slice(20..sps_size + 20);
            let pps_size = u32::from_be_bytes([bytes[sps_size + 20], bytes[sps_size + 21], bytes[sps_size + 22], bytes[sps_size + 23]]) as usize;
            let pps_nal = bytes.slice(sps_size + 24..sps_size + pps_size + 24);

            //Script Tag
            if let Ok((w, h, fr)) = H264::get_width_height_frame_rate(&sps_nal) {
                let mut meta_data = ScriptMetaData::default();
                meta_data.set_height(h as f64);
                meta_data.set_width(w as f64);
                // meta_data.set_videodatarate()
                meta_data.set_videocodecid(7f64); //H.264视频编码的ID通常为 7
                meta_data.set_framerate(fr);
                if let Ok(meta_data_bytes) = meta_data.to_bytes() {
                    let script_header_bytes = TagHeader::build(TagType::Video, ts, meta_data_bytes.len() as u32).to_bytes();
                    let tag_size_bytes = PreviousTagSize::new((script_header_bytes.len() + meta_data_bytes.len()) as u32).previous_tag_size();
                    first_pkg.put(script_header_bytes);
                    first_pkg.put(meta_data_bytes);
                    first_pkg.put(tag_size_bytes);
                }
            }
            //Video Tag[0]
            let con_record = AvcDecoderConfigurationRecord::build(sps_nal, pps_nal);
            let data_tag0_bytes = VideoTagDataFirst::build(con_record).to_bytes();
            let header_tag0_bytes = TagHeader::build(TagType::Video, ts, data_tag0_bytes.len() as u32).to_bytes();
            let tag_size_bytes = PreviousTagSize::new((header_tag0_bytes.len() + data_tag0_bytes.len()) as u32).previous_tag_size();
            first_pkg.put(header_tag0_bytes);
            first_pkg.put(data_tag0_bytes);
            first_pkg.put(tag_size_bytes);
            //sps+pps+...+idr
            first_pkg.put(bytes);
            let _ = flv_tx.send_data(first_pkg.freeze()).await.hand_log(|msg| warn!("{msg}"));
            return;
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
pub async fn send_flv(mut flv_tx: body::Sender, mut rx: broadcast::Receiver<Bytes>) {
    first_frame(&mut flv_tx, &mut rx).await;
    loop {
        match rx.recv().await {
            Ok(bytes) => {
                let _ = flv_tx.send_data(bytes).await.hand_log(|msg| warn!("{msg}"));
            }
            Err(broadcast::error::RecvError::Lagged(_amt)) => {
                rx = rx.resubscribe();
            }
            Err(..) => {}
        }
    }
}