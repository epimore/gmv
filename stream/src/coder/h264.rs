use h264_reader::{Context, rbsp};
use h264_reader::nal::pps::PicParameterSet;
use h264_reader::nal::sps::{SeqParameterSet};
use log::{debug, warn};
use rtp::codecs::h264::H264Packet;
use rtp::packetizer::Depacketizer;

use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;

use crate::coder::{FrameData, HandleFrameDataFn};
use crate::general::mode::Coder;

pub struct H264 {
    handle_fn: HandleFrameDataFn,
    h264packet: H264Packet,
}

impl H264 {
    pub fn init_annexb(handle_fn: HandleFrameDataFn) -> Self {
        Self { handle_fn, h264packet: H264Packet::default() }
    }
    pub fn init_avc(handle_fn: HandleFrameDataFn) -> Self {
        let mut h264packet = H264Packet::default();
        h264packet.is_avc = true;
        Self { handle_fn, h264packet }
    }

    pub fn handle_demuxer(&mut self, payload: Bytes, timestamp: u32) -> GlobalResult<()> {
        let data = self.h264packet.depacketize(&payload).hand_log(|msg| debug!("{msg}"))?;
        if data.len() != 0 {
            let fun = &self.handle_fn;
            fun(FrameData { pay_type: Coder::H264, timestamp, data }).hand_log(|msg| warn!("{msg}"))?;
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
            warn!("fps 未知;使用默认 30");
            30.0
        });
        Ok((width, height, fps))
    }
}

#[cfg(test)]
mod test {
    use common::bytes::Bytes;
    use crate::coder::h264::H264;

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
}