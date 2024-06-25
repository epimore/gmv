use std::collections::VecDeque;
use std::io::Cursor;

use byteorder::ReadBytesExt;
use log::{debug, warn};

use common::anyhow::anyhow;
use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use constructor::New;

use crate::coder::{FrameData, HandleFrameDataFn, parse_vui_parameters, read_uev};

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
    pub fn build(f: HandleFrameDataFn) -> Self {
        Self {
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
                self.hand_aggregation_stapa_naul(bytes, timestamp).hand_log(|msg| warn!("{msg}"))?;
            }
            FUA_NALU_TYPE_28 => {
                self.hand_fua_naul(bytes, timestamp).hand_log(|msg| warn!("{msg}"))?;
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
        // naul.put(&*ANNEXB_NALUSTART_CODE);
        naul.put(bytes);
        let data = naul.freeze();
        let fun = &self.f;
        fun(FrameData::Video { timestamp, data }).hand_log(|msg| warn!("{msg}"))?;
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
            // naul.put(&*ANNEXB_NALUSTART_CODE);
            naul.put(&bytes[curr_offset..curr_offset + nalu_size]);
            curr_offset += nalu_size;
            let data = naul.freeze();
            let fun = &self.f;
            fun(FrameData::Video { timestamp, data }).hand_log(|msg| warn!("{msg}"))?;
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
                // naul.put(&*ANNEXB_NALUSTART_CODE);
                naul.put_u8(nalu_ref_idc | fragmented_nalu_type);
                naul.put(fua_buffer);
                let data = naul.freeze();
                let fun = &self.f;
                fun(FrameData::Video { timestamp, data }).hand_log(|msg| warn!("{msg}"))?;
            }
        }
        Ok(())
    }
}

fn get_frame_type_from_nalu(index: usize, nalu: &[u8]) -> &str {
    let nal_unit_type = nalu[index] & 0x1F; // 获取 NALU 类型
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

//图像参数集(Picture Parameter Set)
pub struct H264PPS {}

// 序列参数集(Sequence Parameter Set)
// 分辨率、帧率、色彩格式
pub struct H264SPS {
    profile_idc: u8,
    constraint_set_flags: u8,
    level_idc: u8,
    seq_parameter_set_id: u32,
    chroma_format_idc: u32,
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
    frame_mbs_only_flag: bool,
    frame_crop_left_offset: u32,
    frame_crop_right_offset: u32,
    frame_crop_top_offset: u32,
    frame_crop_bottom_offset: u32,
    vui_parameters_present_flag: bool,
    num_units_in_tick: Option<u32>,
    time_scale: Option<u32>,
    fixed_frame_rate_flag: bool,
}

impl H264SPS {
    //获取sps关键信息
    //需要跳过分隔符
    pub fn get_sps_info_by_nalu(skip_byte: usize, nalu: &Bytes) -> GlobalResult<Option<Self>> {
        let data = &nalu[skip_byte..];
        let mut reader = Cursor::new(data);
        if reader.read_u8().hand_log(|msg| warn!("{msg}"))? & NALU_TYPE_BITMASK_31 == 7 {
            let profile_idc = reader.read_u8().hand_log(|_| warn!("Failed to read profile_idc"))?;
            let constraint_set_flags = reader.read_u8().hand_log(|_| warn!("Failed to read constraint_set flags"))?;
            let level_idc = reader.read_u8().hand_log(|_| warn!( "Failed to read level_idc"))?;

            let seq_parameter_set_id = read_uev(&mut reader)?;

            let chroma_format_idc = if profile_idc == 100 || profile_idc == 110 || profile_idc == 122 || profile_idc == 244 || profile_idc == 44 || profile_idc == 83 || profile_idc == 86 || profile_idc == 118 || profile_idc == 128 || profile_idc == 138 || profile_idc == 139 || profile_idc == 134 {
                read_uev(&mut reader)?
            } else {
                1 // default value for chroma_format_idc
            };

            // Skip bit depth information if chroma_format_idc is present
            if chroma_format_idc == 3 {
                reader.read_u8().hand_log(|_| warn!( "Failed to read bit_depth_luma_minus8"))?; // bit_depth_luma_minus8
                reader.read_u8().hand_log(|_| warn!( "Failed to read bit_depth_chroma_minus8"))?; // bit_depth_chroma_minus8
            }

            let pic_width_in_mbs_minus1 = read_uev(&mut reader)?;
            let pic_height_in_map_units_minus1 = read_uev(&mut reader)?;
            let frame_mbs_only_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read frame_mbs_only_flag"))? & 0x1 == 1;

            // Frame cropping
            let frame_cropping_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read frame_cropping_flag"))? & 0x1 == 1;
            let (frame_crop_left_offset, frame_crop_right_offset, frame_crop_top_offset, frame_crop_bottom_offset) = if frame_cropping_flag {
                let left_offset = read_uev(&mut reader)?;
                let right_offset = read_uev(&mut reader)?;
                let top_offset = read_uev(&mut reader)?;
                let bottom_offset = read_uev(&mut reader)?;
                (left_offset, right_offset, top_offset, bottom_offset)
            } else {
                (0, 0, 0, 0)
            };

            // VUI parameters
            let vui_parameters_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read vui_parameters_present_flag"))? & 0x1 == 1;
            let (num_units_in_tick, time_scale, fixed_frame_rate_flag) = if vui_parameters_present_flag {
                parse_vui_parameters(&mut reader)?
            } else {
                (None, None, false)
            };


            return Ok(Some(Self {
                profile_idc,
                constraint_set_flags,
                level_idc,
                seq_parameter_set_id,
                chroma_format_idc,
                pic_width_in_mbs_minus1,
                pic_height_in_map_units_minus1,
                frame_mbs_only_flag,
                frame_crop_left_offset,
                frame_crop_right_offset,
                frame_crop_top_offset,
                frame_crop_bottom_offset,
                vui_parameters_present_flag,
                num_units_in_tick,
                time_scale,
                fixed_frame_rate_flag,
            }));
        }
        Ok(None)
    }
    //获取色彩格式、分辨率、帧率
    // chroma_format_idc
    // 0 (Monochrome): 表示视频只有亮度信息（灰度图像），没有色度信息。
    // 1 (4:2:0): 表示色度子采样采用 4:2:0 格式，即色度水平和垂直方向上都减少了一半的分辨率。这是 H.264 中最常见的色度子采样格式，广泛用于压缩高清视频。
    // 2 (4:2:2): 表示色度子采样采用 4:2:2 格式，即色度水平方向上减少了一半的分辨率，但垂直方向上没有减少。这种格式通常用于高质量的视频编辑和制作。
    // 3 (4:4:4): 表示没有色度子采样，色度和亮度具有相同的分辨率。这种格式通常用于高质量的视频存储和传输。
    pub fn get_c_w_h_r(&self) -> (u32, u32, u32, f64) {
        // 计算分辨率
        let width = (self.pic_width_in_mbs_minus1 + 1) * 16 - self.frame_crop_left_offset * 2 - self.frame_crop_right_offset * 2;
        let height = ((self.pic_height_in_map_units_minus1 + 1) * 16) * if self.frame_mbs_only_flag { 1 } else { 2 } - self.frame_crop_top_offset * 2 - self.frame_crop_bottom_offset * 2;
        let mut frame_rate = 0.00f64;
        // 计算帧率
        if let (Some(num_units_in_tick), Some(time_scale)) = (self.num_units_in_tick, self.time_scale) {
            frame_rate = time_scale as f64 / (num_units_in_tick as f64 * 2.0);
        }
        debug!("Width: {}", width);
        debug!("Height: {}", height);
        debug!("Chroma Format IDC: {}", self.chroma_format_idc);
        debug!("Frame Rate: {}", frame_rate);
        (self.chroma_format_idc, width, height, frame_rate)
    }
}

#[test]
fn test() {
    let h264_data = Bytes::from_static(&[
        0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xe0, 0x1f, // SPS
        0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2, // PPS
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x00, 0x0a, 0xb7, 0xb3, 0x0a, 0x3d, 0x4d, 0x40, 0xa0, 0x5f, // IDR
    ]);
    if let Ok(Some(sps)) = H264SPS::get_sps_info_by_nalu(4, &h264_data) {
        println!("{:?}", sps.get_c_w_h_r());
    }
}

