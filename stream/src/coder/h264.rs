use std::collections::VecDeque;
use log::warn;

use common::anyhow::anyhow;
use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use constructor::New;

use crate::coder::{FrameData, HandleFrameDataFn};

pub const STAPA_NALU_TYPE_24: u8 = 24;
pub const STAPB_NALU_TYPE_25: u8 = 25;
pub const MTAP16_NALU_TYPE_26: u8 = 26;
pub const MTAP24_NALU_TYPE_27: u8 = 27;
pub const FUA_NALU_TYPE_28: u8 = 28;
pub const FUB_NALU_TYPE_29: u8 = 29;
pub const SPS_NALU_TYPE_7: u8 = 7;
pub const PPS_NALU_TYPE_8: u8 = 8;
pub const AUD_NALU_TYPE_9: u8 = 9;
pub const FILLER_NALU_TYPE_12: u8 = 12;
pub const FUA_HEADER_SIZE_2: usize = 2;
pub const STAP_MTAP_HEADER_SIZE_1: usize = 1;
pub const STAPA_NALU_LENGTH_SIZE_2: usize = 2;
pub const NALU_DON_LENGTH_SIZE_2: usize = 2;
pub const NALU_TYPE_BITMASK_31: u8 = 0x1F;
pub const NALU_REF_IDC_BITMASK_96: u8 = 0x60;
pub const FU_START_BITMASK_128: u8 = 0x80;
pub const FU_END_BITMASK_64: u8 = 0x40;
pub const OUTPUT_STAP_AHEADER_120: u8 = 0x78;
pub static ANNEXB_NALUSTART_CODE: Bytes = Bytes::from_static(&[0x00, 0x00, 0x00, 0x01]);

//     NAL Unit Header
//     +---------------+
//     |0|1|2|3|4|5|6|7|
//     +-+-+-+-+-+-+-+-+
//     |F|NRI|  Type   |
//     +---------------+
pub struct H264Package {
    f: HandleFrameDataFn,
    fu_buffer: Option<BytesMut>,
    //start_don:<don,byte>
    don_buffer: Option<(u16, VecDeque<(u16, Bytes)>)>,
}

impl H264Package {
    pub fn build(f: HandleFrameDataFn)->Self{
        Self{
            f,
            fu_buffer: None,
            don_buffer: None,
        }
    }

    //NALU : [Start Code] [NALU Header] [NALU Payload]
    //低延时：暂不支持交织模式
    pub fn demuxer_by_rtp_payload(&mut self, bytes: Bytes, timestamp: u32) -> GlobalResult<()> {
        if bytes.len() <= 2 {
            return Err(SysErr(anyhow!("h264 packet is not large enough")));
        }
        let b0 = bytes[0];
        let nalu_type = b0 & NALU_TYPE_BITMASK_31;
        match nalu_type {
            //1-23 NAL unit Single NAL unit packet 5.6
            1..=23 => {
                self.hand_single_naul(bytes, timestamp)?;
            }
            STAPA_NALU_TYPE_24 => {
                self.hand_aggregation_stapa_naul(bytes, timestamp).hand_log_err()?;
            }
            FUA_NALU_TYPE_28 => {
                self.hand_fua_naul(bytes, timestamp).hand_log_err()?;
            }
            tp => {
                warn!("暂不支持nalu type = {tp}");
            }
        }
        Ok(())
    }

    //  0                   1                   2                   3
    //  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // |F|NRI|  Type   |                                               |
    // +-+-+-+-+-+-+-+-+                                               |
    // |                                                               |
    // |               Bytes 2..n of a single NAL unit                 |
    // |                                                               |
    // |                               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    // |                               :...OPTIONAL RTP padding        |
    // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    //     RTP payload format for single NAL unit packet
    fn hand_single_naul(&mut self, bytes: Bytes, timestamp: u32) -> GlobalResult<()> {
        let mut naul = BytesMut::new();
        naul.put(&*ANNEXB_NALUSTART_CODE);
        naul.put(bytes);
        let data = naul.freeze();
        let fun = &self.f;
        fun(FrameData::Video { timestamp, data }).hand_log_err()?;
        Ok(())
    }

    //      0                   1                   2                   3
    //      0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                          RTP Header                           |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |STAP-A NAL HDR |         NALU 1 Size           | NALU 1 HDR    |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                         NALU 1 Data                           |
    //     :                                                               :
    //     +               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |               | NALU 2 Size                   | NALU 2 HDR    |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                         NALU 2 Data                           |
    //     :                                                               :
    //     |                               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                               :...OPTIONAL RTP padding        |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //      An example of an RTP packet including an STAP-A
    //      containing two single-time aggregation units
    fn hand_aggregation_stapa_naul(&mut self, bytes: Bytes, timestamp: u32) -> GlobalResult<()> {
        let mut curr_offset = STAP_MTAP_HEADER_SIZE_1;
        while curr_offset < bytes.len() {
            //nalu size => 2 个byte => u16
            //第一个字节是高位：左移8位，其结果为乘以256；然后位或第二个字节，其结果为相加；最终得到nalu size;
            let nalu_size = ((bytes[curr_offset] as usize) << 8) | bytes[curr_offset + 1] as usize;
            curr_offset += STAPA_NALU_LENGTH_SIZE_2;
            if curr_offset > bytes.len() {
                return Err(SysErr(anyhow!("STAPA declared size is large than rtp payload size")));
            }
            let mut naul = BytesMut::new();
            naul.put(&*ANNEXB_NALUSTART_CODE);
            naul.put(&bytes[curr_offset..curr_offset + nalu_size]);
            curr_offset += nalu_size;
            let data = naul.freeze();
            let fun = &self.f;
            fun(FrameData::Video { timestamp, data }).hand_log_err()?;
        }
        Ok(())
    }

    //      0                   1                   2                   3
    //      0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                          RTP Header                           |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |STAP-B NAL HDR | DON                           | NALU 1 Size   |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     | NALU 1 Size   | NALU 1 HDR    | NALU 1 Data                   |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+                               +
    //     :                                                               :
    //     +               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |               | NALU 2 Size                   | NALU 2 HDR    |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                       NALU 2 Data                             |
    //     :                                                               :
    //     |                               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                               :...OPTIONAL RTP padding        |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    //      An example of an RTP packet including an STAP-B
    //      containing two single-time aggregation units
    fn hand_aggregation_stapb_naul(&mut self, bytes: Bytes, timestamp: u32) -> GlobalResult<()> {
        unimplemented!()
    }


    //0                   1                   2                   3
    //      0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     | FU indicator  |   FU header   |                               |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+                               |
    //     |                                                               |
    //     |                         FU payload                            |
    //     |                                                               |
    //     |                               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //     |                               :...OPTIONAL RTP padding        |
    //     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    //
    //     RTP payload format for FU-A
    fn hand_fua_naul(&mut self, bytes: Bytes, timestamp: u32) -> GlobalResult<()> {
        match &mut self.fu_buffer {
            None => {
                let mut buffer = BytesMut::new();
                buffer.put(&bytes[FUA_HEADER_SIZE_2..]);
                self.fu_buffer = Some(buffer);
            }
            Some(fua_buffer) => {
                fua_buffer.put(&bytes[FUA_HEADER_SIZE_2..]);
            }
        }
        let b1 = bytes[1];
        if b1 & FU_END_BITMASK_64 != 0 {
            let nalu_ref_idc = bytes[0] & NALU_REF_IDC_BITMASK_96;
            let fragmented_nalu_type = b1 & NALU_TYPE_BITMASK_31;

            if let Some(fua_buffer) = self.fu_buffer.take() {
                let mut naul = BytesMut::new();
                naul.put(&*ANNEXB_NALUSTART_CODE);
                naul.put_u8(nalu_ref_idc | fragmented_nalu_type);
                naul.put(fua_buffer);
                let data = naul.freeze();
                let fun = &self.f;
                fun(FrameData::Video { timestamp, data }).hand_log_err()?;
            }
        }
        Ok(())
    }
}
fn get_frame_type_from_nalu(nalu: &[u8]) -> &str {
    let nal_unit_type = nalu[0] & 0x1F; // 获取 NALU 类型
    match nal_unit_type {
        1 => "Non-IDR (P/B Frame)", // 非 IDR 帧（P 帧或 B 帧）
        5 => "IDR (I Frame)",       // IDR 帧（I 帧）
        7 => "SPS (Sequence Parameter Set)",  // 序列参数集
        8 => "PPS (Picture Parameter Set)",   // 图像参数集
        6 => "SEI (Supplemental Enhancement Information)", // 补充增强信息
        9 => "AUD (Access Unit Delimiter)",   // 访问单元分界符
        _ => "Other",               // 其他类型
    }
}