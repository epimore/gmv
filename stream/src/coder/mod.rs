use byteorder::{BigEndian, ReadBytesExt};
use log::warn;

use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use crate::coder::h264::H264;
use crate::general::mode::Coder;

pub mod h264;


#[derive(Clone)]
pub struct FrameData {
    pub pay_type: Coder,
    pub timestamp: u32,
    pub data: Bytes,
}

pub type HandleFrameDataFn = Box<dyn Fn(FrameData) -> GlobalResult<()> + Send + Sync>;

pub struct MediaCoder {
    pub h264: H264,
    // pub h265:H265,
    // pub aac:Aac,
}

impl MediaCoder {
    pub fn register_all(handle_fn: HandleFrameDataFn) -> Self {
        Self { h264: H264::init_avc(handle_fn) }
    }
}

pub fn read_uev<R: std::io::Read>(reader: &mut R) -> GlobalResult<u32> {
    let mut leading_zero_bits = 0;
    while reader.read_u8().hand_log(|msg| warn!("{msg}"))? == 0 {
        leading_zero_bits += 1;
    }

    let mut code_num = 1;
    for _ in 0..leading_zero_bits {
        code_num = (code_num << 1) | reader.read_u8().hand_log(|msg| warn!("{msg}"))?;
    }

    Ok((code_num - 1) as u32)
}

// Helper function to parse VUI parameters
pub fn parse_vui_parameters<R: std::io::Read>(reader: &mut R) -> GlobalResult<(Option<u32>, Option<u32>, bool)> {
    // Parse VUI parameters according to H.264 specification
    // This is a simplified version, you may need to parse more fields depending on your needs
    let aspect_ratio_info_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read aspect_ratio_info_present_flag"))? & 0x1 == 1;
    if aspect_ratio_info_present_flag {
        // Skip aspect_ratio_idc and related fields
        reader.read_u8().hand_log(|_| warn!( "Failed to read aspect_ratio_idc"))?;
    }

    let overscan_info_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read overscan_info_present_flag"))? & 0x1 == 1;
    if overscan_info_present_flag {
        // Skip overscan_appropriate_flag
        reader.read_u8().hand_log(|_| warn!( "Failed to read overscan_appropriate_flag"))?;
    }

    let video_signal_type_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read video_signal_type_present_flag"))? & 0x1 == 1;
    if video_signal_type_present_flag {
        // Skip video_format, video_full_range_flag, and colour_description_present_flag
        reader.read_u8().hand_log(|_| warn!( "Failed to read video_format"))?;
    }

    let chroma_loc_info_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read chroma_loc_info_present_flag"))? & 0x1 == 1;
    if chroma_loc_info_present_flag {
        // Skip chroma_sample_loc_type_top_field and chroma_sample_loc_type_bottom_field
        read_uev(reader)?; // chroma_sample_loc_type_top_field
        read_uev(reader)?; // chroma_sample_loc_type_bottom_field
    }

    let timing_info_present_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read timing_info_present_flag"))? & 0x1 == 1;
    if timing_info_present_flag {
        let num_units_in_tick = reader.read_u32::<BigEndian>().hand_log(|_| warn!( "Failed to read num_units_in_tick"))?;
        let time_scale = reader.read_u32::<BigEndian>().hand_log(|_| warn!( "Failed to read time_scale"))?;
        let fixed_frame_rate_flag = reader.read_u8().hand_log(|_| warn!( "Failed to read fixed_frame_rate_flag"))? & 0x1 == 1;
        return Ok((Some(num_units_in_tick), Some(time_scale), fixed_frame_rate_flag));
    }

    Ok((None, None, false))
}
