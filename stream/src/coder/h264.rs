use std::io::{Cursor};
use byteorder::{BigEndian, ByteOrder};
use h264_reader::{Context, rbsp};
use h264_reader::nal::pps::PicParameterSet;
use h264_reader::nal::sps::SeqParameterSet;
use log::error;
use memchr::memmem;
use common::log::{warn};
use rtp::codecs::h264::H264Packet;
use rtp::packetizer::Depacketizer;

use common::anyhow::anyhow;
use common::bytes::{Buf, BufMut, Bytes, BytesMut};
use common::exception::{GlobalError, GlobalResult, TransError};
use common::exception::GlobalError::SysErr;

use crate::coder::{FrameData, HandleFrame};
use crate::general::mode::Coder;

pub struct H264 {
    flv_tx: Option<crossbeam_channel::Sender<FrameData>>,
    hls_tx: Option<crossbeam_channel::Sender<FrameData>>,
    h264packet: H264Packet,
}

impl HandleFrame for H264 {
    fn next_step(&self, frame_data: FrameData) -> GlobalResult<()> {
        match (&self.flv_tx, &self.hls_tx) {
            (Some(flv_tx), Some(hls_tx)) => {
                flv_tx.send(frame_data.clone()).hand_log(|msg| error!("{msg}"))?;
                hls_tx.send(frame_data).hand_log(|msg| error!("{msg}"))?;
            }
            (Some(flv_tx), None) => {
                flv_tx.send(frame_data).hand_log(|msg| error!("{msg}"))?;
            }
            (None, Some(hls_tx)) => {
                hls_tx.send(frame_data).hand_log(|msg| error!("{msg}"))?;
            }
            (None, None) => {
                Err(GlobalError::new_sys_error("无frame发送端", |msg| error!("{msg}")))?;
            }
        }
        Ok(())
    }
}

impl H264 {
    pub fn is_new_access_unit(nal_type: u8, first_mb: u8) -> bool {
        if matches!(nal_type,6..=9|14..=18) {
            return true;
        }
        if matches!(nal_type,1|2|5) {
            return first_mb != 0;
        }
        false
    }

    pub fn init_annexb(flv_tx: Option<crossbeam_channel::Sender<FrameData>>, hls_tx: Option<crossbeam_channel::Sender<FrameData>>) -> Self {
        Self { flv_tx, hls_tx, h264packet: H264Packet::default() }
    }
    pub fn init_avc(flv_tx: Option<crossbeam_channel::Sender<FrameData>>, hls_tx: Option<crossbeam_channel::Sender<FrameData>>) -> Self {
        let mut h264packet = H264Packet::default();
        h264packet.is_avc = true;
        Self { flv_tx, hls_tx, h264packet }
    }

    pub fn handle_demuxer(&mut self, payload: Bytes, timestamp: u32) -> GlobalResult<()> {
        let raw_data = self.h264packet.depacketize(&payload).hand_log(|msg| warn!("{msg}"))?;
        let raw_data_len = raw_data.len();
        let nal_data_size_len = 4;
        let mut curr_offset = 0;
        while curr_offset + nal_data_size_len < raw_data_len {
            let size_data = raw_data.slice(curr_offset..curr_offset + nal_data_size_len);
            let size_data_len = BigEndian::read_u32(size_data.as_ref()) as usize;
            let last_offset = curr_offset + nal_data_size_len + size_data_len;
            if last_offset > raw_data_len {
                return Err(GlobalError::new_sys_error("nal size larger than raw buffer", |msg| warn!("{msg}")));
            } else {
                let nal_data = raw_data.slice(curr_offset..last_offset);
                let frame_data = FrameData { pay_type: Coder::H264(None, None, false), timestamp, data: nal_data };
                self.next_step(frame_data)?;
                curr_offset = last_offset;
            }
        }
        Ok(())
    }

    pub fn parse_sps(sps_nal: &Bytes) -> GlobalResult<SeqParameterSet> {
        let sps_rbsp = rbsp::decode_nal(&sps_nal[..]).hand_log(|msg| warn!("{msg}"))?;
        let sps = SeqParameterSet::from_bits(rbsp::BitReader::new(&*sps_rbsp))
            .map_err(|err| SysErr(anyhow!("{:?}",err)))
            .hand_log(|msg| warn!("{msg}"))?;
        Ok(sps)
    }
    pub fn parse_sps_pps(pps_nal: &Bytes, sps_nal: &Bytes) -> GlobalResult<(SeqParameterSet, PicParameterSet)> {
        let sps_rbsp = rbsp::decode_nal(&sps_nal[..]).hand_log(|msg| warn!("{msg}"))?;
        let sps = SeqParameterSet::from_bits(rbsp::BitReader::new(&*sps_rbsp))
            .map_err(|err| SysErr(anyhow!("{:?}",err)))
            .hand_log(|msg| warn!("{msg}"))?;
        let mut ctx = Context::default();
        ctx.put_seq_param_set(sps.clone());
        let pps_rbsp = rbsp::decode_nal(&pps_nal[..]).hand_log(|msg| warn!("{msg}"))?;
        let pps = PicParameterSet::from_bits(&ctx, rbsp::BitReader::new(&*pps_rbsp))
            .map_err(|err| SysErr(anyhow!("{:?}",err)))
            .hand_log(|msg| warn!("{msg}"))?;
        Ok((sps, pps))
    }

    pub fn get_width_height_frame_rate(sps_nal: &Bytes) -> GlobalResult<(u32, u32, f64)> {
        let sps_rbsp = rbsp::decode_nal(&sps_nal[..]).hand_log(|msg| warn!("{msg}"))?;
        let sps = SeqParameterSet::from_bits(rbsp::BitReader::new(&*sps_rbsp))
            .map_err(|err| SysErr(anyhow!("{:?}",err)))
            .hand_log(|msg| warn!("{msg}"))?;
        let (width, height) = sps.pixel_dimensions()
            .map_err(|err| SysErr(anyhow!("{:?}",err)))
            .hand_log(|msg| warn!("{msg}"))?;
        let fps = sps.fps().unwrap_or_else(|| {
            warn!("fps 未知;使用默认 25");
            25.0
        });
        Ok((width, height, fps))
    }

    #[allow(non_upper_case_globals)]
    pub fn extract_nal_annexb_to_len(nals: &mut Vec<Bytes>, bytes_annexb: Bytes) -> GlobalResult<()> {
        if !bytes_annexb.starts_with(&[0]) {
            return Err(GlobalError::new_sys_error("h264 invalid start annexb code", |msg| warn!("{msg}")));
        }
        const annexb: [u8; 3] = [0x00u8, 00, 01];
        // let mut nals = Vec::new();
        let mut start = 3usize;
        let mut iter = memmem::find_iter(&*bytes_annexb, &annexb);
        if let Some(index) = iter.next() {
            if index == 1 {
                start = 4;
            } else if index > 1 {
                return Err(GlobalError::new_sys_error("h264 invalid start annexb code", |msg| warn!("{msg}")));
            }
        }
        while let Some(index) = iter.next() {
            let mut end = index;
            if bytes_annexb[index - 1] == 0 {
                end -= 1;
            }
            let mut nal = BytesMut::new();

            // info!("nalu type = {:02x},scope = {:02x?}",bytes_annexb[start],bytes_annexb[start-3..start+10].to_vec());

            nal.put_u32((end - start) as u32);
            nal.put_slice(&bytes_annexb[start..end]);
            // let nal = Bytes::copy_from_slice(&bytes_annexb[start..end]);
            nals.push(nal.freeze());
            start = index + 3;
        }

        // info!("nalu type = {:02x},scope = {:02x?}",bytes_annexb[start],bytes_annexb[start-3..start+10].to_vec());

        let mut nal = BytesMut::new();
        nal.put_u32((bytes_annexb.len() - start) as u32);
        nal.put_slice(&bytes_annexb[start..bytes_annexb.len()]);
        // let nal = Bytes::copy_from_slice(&bytes_annexb[start..bytes_annexb.len()]);
        nals.push(nal.freeze());
        Ok(())
    }

    pub fn extract_nal_by_annexb1(bytes_annexb: Bytes) -> Vec<Bytes> {
        let len = bytes_annexb.len() as u64;
        let mut nals = Vec::new();
        let mut nal = BytesMut::new();
        let mut cursor = Cursor::new(bytes_annexb);
        let mut count_zero = 0u8;
        while cursor.position() < len {
            let val = cursor.get_u8();
            match val {
                0 => {
                    if count_zero == 3 {
                        nal.put_u8(val);
                    } else {
                        count_zero += 1;
                    }
                }
                1 => {
                    match count_zero {
                        0 => {
                            nal.put_u8(val);
                        }
                        1 => {
                            nal.put_u8(0);
                            nal.put_u8(val);
                            count_zero = 0;
                        }
                        _ => {
                            if nal.len() > 0 {
                                let bytes_mut = std::mem::take(&mut nal);
                                nals.push(bytes_mut.freeze());
                            }
                            count_zero = 0;
                        }
                    }
                }
                _ => {
                    while count_zero > 0 {
                        nal.put_u8(0);
                        count_zero -= 1;
                    }
                    nal.put_u8(val);
                }
            }
        }
        while count_zero > 0 {
            nal.put_u8(0);
            count_zero -= 1;
        }
        if nal.len() > 0 {
            nals.push(nal.freeze());
        }
        nals
    }
}

#[cfg(test)]
mod test {
    use memchr::memmem;
    use common::bytes::Bytes;

    use crate::coder::h264::H264;
    use crate::container::ps::PsPacket;

    #[test]
    fn test_sps() {
        let sps = [
            0x67, 0x64, 0x00, 0x0c, 0xac, 0x3b, 0x50, 0xb0,
            0x4b, 0x42, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00,
            0x00, 0x03, 0x00, 0x3d, 0x08,
        ];
        println!("{:?}", H264::get_width_height_frame_rate(&Bytes::from(sps.to_vec())));

        let sps = [
            0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50,
            0x05, 0xbb, 0x01, 0x6c, 0x80, 0x00, 0x00, 0x03,
            0x00, 0x80, 0x00, 0x00, 0x1e, 0x07, 0x8c, 0x18,
            0xcb,
        ];
        println!("{:?}", H264::get_width_height_frame_rate(&Bytes::from(sps.to_vec())));

        let sps = [
            0x67, 0x42, 0xc0, 0x28, 0xd9, 0x00, 0x78, 0x02,
            0x27, 0xe5, 0x84, 0x00, 0x00, 0x03, 0x00, 0x04,
            0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60, 0xc9, 0x20,
        ];
        println!("{:?}", H264::get_width_height_frame_rate(&Bytes::from(sps.to_vec())));

        let sps = [
            0x67, 0x64, 0x00, 0x28, 0xac, 0xd9, 0x40, 0x78,
            0x02, 0x27, 0xe5, 0x84, 0x00, 0x00, 0x03, 0x00,
            0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60,
            0xc6, 0x58,
        ];
        println!("{:?}", H264::get_width_height_frame_rate(&Bytes::from(sps.to_vec())));
    }

    #[test]
    fn test_if_else() {
        let a = 2;
        let b = true;
        if a == 2 {
            println!("a==2");
        } else if b {
            println!("a != 2 and b is true");
        }
        println!("end");
    }

    #[test]
    fn test_extract_nal_by_annexb() {
        let bytes_annexb = Bytes::from_static(&[
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1e,
            0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xf2,
            0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x0a, 0x02,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x0a, 0x00,
        ]);

        let mut vec = Vec::new();
        H264::extract_nal_annexb_to_len(&mut vec, bytes_annexb).unwrap();
        vec.iter().map(|iter| println!("{:02x?}", iter.to_vec())).count();
    }

    #[test]
    fn test_extract_nal_by_annexb1() {
        let bytes_annexb = Bytes::from_static(&[
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1e,
            0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xf2,
            0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x0a, 0x02,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x0a, 0x00,
        ]);
        let vec = H264::extract_nal_by_annexb1(bytes_annexb);
        vec.iter().map(|iter| println!("{:02x?}", iter.to_vec())).count();
    }

    // #[test]
    // fn test_es() {
    //     let input = include_bytes!("/mnt/e/code/rust/study/media/rsmpeg/tests/assets/vids/es1.dump");
    //     let bytes = Bytes::copy_from_slice(input);
    //     let mut vec = Vec::new();
    //     H264::extract_nal_annexb_to_len(&mut vec, bytes).unwrap();
    //     println!("vec len = {}", vec.len());
    //     vec.iter().map(|iter| {
    //         if iter.len() <= 2 {
    //             println!("1111");
    //         }
    //     }
    //     ).count();
    // }
}