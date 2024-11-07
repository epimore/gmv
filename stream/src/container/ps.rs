use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use byteorder::{BigEndian, ReadBytesExt};
use memchr::memmem;

use common::anyhow::anyhow;
use common::bytes::{BufMut};
use common::bytes::{Bytes, BytesMut};
use common::exception::{GlobalError, GlobalResult, TransError};
use common::exception::GlobalError::SysErr;
use common::log::{info, warn};
use common::tokio::sync::broadcast;
use crate::coder::h264::H264;
use crate::coder::{FrameData, HandleFrame};
use crate::general::mode::Coder;

const PS_PACK_START_CODE: u32 = 0x000001BA;
const PS_PACK_START_IDENT: u8 = 0xBA;
const PS_SYS_START_CODE: u32 = 0x000001BB;
const PS_SYS_MAP_START_CODE: u32 = 0x000001BC;
const PS_PES_START_CODE_VIDEO_FIRST: u8 = 0xE0;
const PS_PES_START_CODE_VIDEO_LAST: u8 = 0xEF;
const PS_PES_START_CODE_AUDIO_FIRST: u8 = 0xC0;
const PS_PES_START_CODE_AUDIO_LAST: u8 = 0xDF;
const PS_BASE_LEN: usize = 6; //ps len min = pes header
// const PS_HEADER_BASE_LEN: usize = 14; //ps header
const PS_START_CODE_PREFIX: [u8; 3] = [0x00, 0x00, 0x01u8];

// #[derive(Default)]
pub struct Ps {
    tx: broadcast::Sender<FrameData>,
    pub ps_packet: PsPacket,
}

impl HandleFrame for Ps {
    fn next_step(&self, frame_data: FrameData) -> GlobalResult<()> {
        self.tx.send(frame_data)
            .map_err(|err| GlobalError::new_biz_error(1199, &err.to_string(), |msg| info!("http handler 关闭通道:{msg}")))
            .map(|_| ())
    }
}

impl Ps {
    pub fn init(tx: broadcast::Sender<FrameData>) -> Self {
        Self {
            tx,
            ps_packet: PsPacket::default(),
        }
    }
    pub fn handle_demuxer(&mut self, marker: bool, timestamp: u32, bytes: Bytes) -> GlobalResult<()> {
        if let Some(raws) = self.ps_packet.parse(marker, bytes)? {
            for raw in raws {
                let frame_data = FrameData { pay_type: Coder::H264(None, None, false), timestamp, data: raw };
                self.next_step(frame_data)?;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct PsPacket {
    ps_header: Option<PsHeader>,
    ps_sys_header: Option<PsSysHeader>,
    ps_sys_map: Option<PsSysMap>,
    payload: BytesMut,
}

impl PsPacket {
    pub fn parse(&mut self, marker: bool, bytes: Bytes) -> GlobalResult<Option<Vec<Bytes>>> {
        self.payload.put(bytes);
        if marker {
            let payload = std::mem::take(&mut self.payload).freeze();
            let len = (payload.len() - PS_BASE_LEN) as u64;
            let mut cursor = Cursor::new(payload);
            let mut pes_packets = Vec::new();
            while cursor.position() < len {
                let mut start_code = [0u8; 3];
                cursor.read_exact(&mut start_code).hand_log(|msg| warn!("{msg}"))?;
                if start_code == PS_START_CODE_PREFIX {
                    let ident = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
                    match ident {
                        PS_PACK_START_IDENT => {
                            let ps_header = PsHeader::parse(&mut cursor).hand_log(|msg| warn!("{msg}"))?;
                            self.ps_header = Some(ps_header);
                            let sys_start_code = cursor.read_u32::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
                            if sys_start_code == PS_SYS_START_CODE {
                                let ps_sys_header = PsSysHeader::parse(&mut cursor).hand_log(|msg| warn!("{msg}"))?;
                                let ps_sys_map = PsSysMap::parse(&mut cursor).hand_log(|msg| warn!("{msg}"))?;
                                self.ps_sys_header = Some(ps_sys_header);
                                self.ps_sys_map = Some(ps_sys_map);
                            } else {
                                cursor.seek(SeekFrom::Current(-4)).hand_log(|msg| warn!("{msg}"))?;
                            }
                        }
                        PS_PES_START_CODE_VIDEO_FIRST..=PS_PES_START_CODE_VIDEO_LAST => {
                            if let Some(pes_pkt) = PesPacket::read_video_pes_data(&mut cursor, ident).hand_log(|msg| warn!("{msg}"))? {
                                pes_packets.push(pes_pkt);
                            }
                        }
                        PS_PES_START_CODE_AUDIO_FIRST..=PS_PES_START_CODE_AUDIO_LAST => {
                            PesPacket::read_audio_pes_data(&mut cursor).hand_log(|msg| warn!("{msg}"))?;
                        }
                        _ => {
                            return Err(GlobalError::new_sys_error("invalid data:ps start code ident is not 0xba|0xe0|0xc0..", |msg| warn!("{msg}")));
                        }
                    }
                } else {
                    return Err(GlobalError::new_sys_error(&format!("invalid data:ps_start_code_prefix is not 0x000001,val = {:02x?}", start_code), |msg| warn!("{msg}")));
                }
            }
            let vec = self.parse_to_nalu(pes_packets).hand_log(|msg| warn!("{msg}"))?;
            return Ok(Some(vec));
        }
        Ok(None)
    }
    fn parse_to_nalu(&self, pes_packets: Vec<PesPacket>) -> GlobalResult<Vec<Bytes>> {
        let mut nalus = Vec::new();
        if let Some(sys_map) = &self.ps_sys_map {
            let mut payload = BytesMut::new();
            for pes_packet in pes_packets {
                let stream_type = &sys_map.es_map_info.get(&pes_packet.stream_id).ok_or_else(|| SysErr(anyhow!("stream id in es not found in ps sys map."))).hand_log(|msg| warn!("{msg}"))?.stream_type;
                match stream_type {
                    //H264
                    &0x1B => {
                        match pes_packet.pes_inner_data {
                            PesInnerData::PesPtsDtsInfo(PesPtsDtsInfo { pes_payload, .. }) => {
                                payload.put(pes_payload);
                            }
                            PesInnerData::PesAllData(pes_payload) => {
                                payload.put(pes_payload);
                            }
                            PesInnerData::PesAllPadding(_) => {}
                        }
                    }
                    /* //MPEG-4
                     &0x10 => {}
                     //SVAC-VIDEO
                     &0x80 => {}
                     //H265
                     &0x24 => {}
                     //G711-A
                     &0x90 => {}
                     //G711-U
                     &0x91 => {}
                     //G722-1
                     &0x92 => {}
                     //G723-1
                     &0x93 => {}
                     //G729
                     &0x99 => {}
                     //SVAC-AUDIO
                     &0x9B => {}
                     //AAC
                     &0x0F => {}*/
                    &_ => {
                        return Err(GlobalError::new_sys_error(&format!("系统暂不支持类型：{stream_type}"), |msg| warn!("{msg}")));
                    }
                };
            }
            if payload.len() > 4 {
                H264::extract_nal_annexb_to_len(&mut nalus, payload.freeze()).hand_log(|msg| warn!("{msg}"))?;
            }
        }
        Ok(nalus)
    }
}

/*
| Name                  | Number of bits | Description                                                                                                  |
|-----------------------|----------------|--------------------------------------------------------------------------------------------------------------|
| sync bytes            | 32             | 0x000001BA                                                                                                   |
| marker bits           | 2              | 01b for MPEG-2 version. The marker bits for the MPEG-1 version are 4 bits with value 0010b.                  |
| System clock [32..30] | 3              | System Clock Reference (SCR) bits 32 to 30                                                                   |
| marker bit            | 1              | 1 bit always set.                                                                                           |
| System clock [29..15] | 15             | System clock bits 29 to 15                                                                                   |
| marker bit            | 1              | 1 bit always set.                                                                                           |
| System clock [14..0]  | 15             | System clock bits 14 to 0                                                                                   |
| marker bit            | 1              | 1 bit always set.                                                                                           |
| SCR extension         | 9              | SCR extension                                                                                               |
| marker bit            | 1              | 1 bit always set.                                                                                           |
| bit rate              | 22             | In units of 50 bytes per second.                                                                             |
| marker bits           | 2              | 11 bits always set.                                                                                         |
| reserved              | 5              | Reserved for future use.                                                                                    |
| stuffing length       | 3              | Stuffing length                                                                                             |
| stuffing bytes        | 8 * stuffing length |                                                                                                         |
| system header (optional) | 0 or more   | If system header start code follows: 0x000001BB                                                              |

// -----0---------|--------1------|--------2------|--------3------|--------4------|-------5-------
// 7 6 5 4 3 2 1 0 7 6 5 4 3 2 1 0 7 6 5 4 3 2 1 0 7 6 5 4 3 2 1 0 7 6 5 4 3 2 1 0 7 6 5 4 3 2 1 0
// VER|SCR_B|M|        SCR_B:[29..15]       |M|        SCR_B:[14..0]        |M|      SCR_E      |M
//    |32-30|
*/
#[allow(dead_code)]
pub struct PsHeader {
    start_code: u32,
    ver_system_clock_reference_base_marker: [u8; 6],
    program_mux_rate22_marker_bit1_x2: [u8; 3],
    reserved5_pack_stuffing_length3: u8,
    stuffing_byte: Bytes,
}

impl PsHeader {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let mut ver_system_clock_reference_base_marker = [0u8; 6];
        cursor.read_exact(&mut ver_system_clock_reference_base_marker).hand_log(|msg| warn!("{msg}"))?;
        let mut program_mux_rate22_marker_bit1_x2 = [0u8; 3];
        cursor.read_exact(&mut program_mux_rate22_marker_bit1_x2).hand_log(|msg| warn!("{msg}"))?;
        let reserved5_pack_stuffing_length3 = cursor.read_u8().hand_log(|msg| warn!("{msg}"))? & 0b0000_0111u8;
        let mut mut_stuffing_byte = BytesMut::with_capacity(reserved5_pack_stuffing_length3 as usize);
        unsafe { mut_stuffing_byte.set_len(reserved5_pack_stuffing_length3 as usize); }
        if reserved5_pack_stuffing_length3 > 0 {
            cursor.read_exact(&mut *mut_stuffing_byte).hand_log(|msg| warn!("{msg}"))?;
        }
        let ps_header = Self {
            start_code: PS_PACK_START_CODE,
            ver_system_clock_reference_base_marker,
            program_mux_rate22_marker_bit1_x2,
            reserved5_pack_stuffing_length3,
            stuffing_byte: mut_stuffing_byte.freeze(),
        };
        Ok(ps_header)
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PsSysHeader {
    start_code: u32,
    len: u16,
    rate_audio_video_band_flag: [u8; 6],
    ps_stream_vec: Vec<PsStream>,
}

impl PsSysHeader {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let len = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let index = cursor.position() + len as u64;
        let mut rate_audio_video_band_flag = [0u8; 6];
        cursor.read_exact(&mut rate_audio_video_band_flag).hand_log(|msg| warn!("{msg}"))?;
        let mut ps_stream_vec = Vec::new();
        while cursor.position() < index {
            let stream_id = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
            if stream_id >> 7 == 1 {
                let mut p_psd = [0u8; 2];
                cursor.read_exact(&mut p_psd).hand_log(|msg| warn!("{msg}"))?;
                let ps_stream = PsStream { stream_id, p_psd };
                ps_stream_vec.push(ps_stream);
            } else {
                break;
            }
        }
        cursor.seek(SeekFrom::Start(index)).hand_log(|msg| warn!("{msg}"))?;
        Ok(Self {
            start_code: PS_SYS_START_CODE,
            len,
            rate_audio_video_band_flag,
            ps_stream_vec,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PsStream {
    stream_id: u8,
    p_psd: [u8; 2],
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PsSysMap {
    start_code3_map_stream_id8: u32,
    ps_map_length: u16,
    indicator1_reserved2_version5: u8,
    reserved7_marker1: u8,
    ps_info_length: u16,
    ps_info_descriptor: DescriptorUnType,
    es_map_length: u16,
    es_map_info: HashMap<u8, EsInfo>,
    crc_32: u32,
}

impl PsSysMap {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let start_code3_map_stream_id8 = cursor.read_u32::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        if start_code3_map_stream_id8 != PS_SYS_MAP_START_CODE {
            return Err(GlobalError::new_sys_error("invalid ps_sys_map_start_code", |msg| warn!("{msg}")));
        }
        let ps_map_length = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let indicator1_reserved2_version5 = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
        let reserved7_marker1 = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
        let ps_info_length = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let ps_info_descriptor = DescriptorUnType::parse(cursor, ps_info_length as usize).hand_log(|msg| warn!("{msg}"))?;

        // let ps_info_index = cursor.position() + ps_info_length as u64;
        // let mut ps_info_descriptor = Vec::new();
        // while cursor.position() < ps_info_index {
        //     let descriptor = Descriptor::parse(cursor).hand_log(|msg|warn!("{msg}"))?;
        //     ps_info_descriptor.push(descriptor);
        // }
        let es_map_length = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let es_map_index = cursor.position() + es_map_length as u64;
        let mut es_map_info = HashMap::new();
        while cursor.position() < es_map_index {
            let (es_id, es_info) = EsInfo::parse(cursor).hand_log(|msg| warn!("{msg}"))?;
            es_map_info.insert(es_id, es_info);
        }
        let crc_32 = cursor.read_u32::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        Ok(Self {
            start_code3_map_stream_id8,
            ps_map_length,
            indicator1_reserved2_version5,
            reserved7_marker1,
            ps_info_length,
            ps_info_descriptor,
            es_map_length,
            es_map_info,
            crc_32,
        })
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct EsInfo {
    stream_type: u8,
    es_info_length: u16,
    es_info_descriptor: DescriptorUnType,
}

impl EsInfo {
    //stream_id:EsInfo
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<(u8, Self)> {
        let stream_type = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
        let es_id = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
        let es_info_length = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let es_info_descriptor = DescriptorUnType::parse(cursor, es_info_length as usize).hand_log(|msg| warn!("{msg}"))?;
        // let es_info_index = cursor.position() + es_info_length as u64;
        // let mut es_info_descriptor = Vec::new();
        // while cursor.position() < es_info_index {
        //     let descriptor = Descriptor::parse(cursor).hand_log(|msg|warn!("{msg}"))?;
        //     es_info_descriptor.push(descriptor);
        // }
        Ok((es_id, Self {
            stream_type,
            es_info_length,
            es_info_descriptor,
        }))
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct DescriptorUnType(Bytes);


impl DescriptorUnType {
    fn parse(cursor: &mut Cursor<Bytes>, descriptor_length: usize) -> GlobalResult<Self> {
        let mut descriptor_data = BytesMut::with_capacity(descriptor_length);
        unsafe { descriptor_data.set_len(descriptor_length); }
        cursor.read_exact(&mut *descriptor_data).hand_log(|msg| warn!("{msg}"))?;
        Ok(Self(descriptor_data.freeze()))
    }
}
//
// #[derive(Debug)]
// #[allow(dead_code)]
// pub struct Descriptor {
//     descriptor_tag: u8,
//     descriptor_length: u8,
//     descriptor_data: Bytes,
// }
//
// impl Descriptor {
//     pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
//         let descriptor_tag = cursor.read_u8().hand_log(|msg|warn!("{msg}"))?;
//         let descriptor_length = cursor.read_u8().hand_log(|msg|warn!("{msg}"))?;
//         let mut descriptor_data = BytesMut::with_capacity(descriptor_length as usize);
//         unsafe { descriptor_data.set_len(descriptor_length as usize); }
//         cursor.read_exact(&mut *descriptor_data).hand_log(|msg|warn!("{msg}"))?;
//         Ok(Descriptor {
//             descriptor_tag,
//             descriptor_length,
//             descriptor_data: descriptor_data.freeze(),
//         })
//     }
// }

#[allow(dead_code)]
pub struct PesPacket {
    start_code_prefix: [u8; 3],
    stream_id: u8,
    packet_len: u16,
    pes_inner_data: PesInnerData,
}

impl PesPacket {
    pub fn parse_data(bytes: Bytes) -> GlobalResult<Vec<Self>> {
        let data_len = bytes.len() as u64;
        let mut cursor = Cursor::new(bytes);
        let mut pes_packets = Vec::new();
        let index = data_len - PS_BASE_LEN as u64;
        while cursor.position() < index {
            let mut start_code_prefix = [0u8; 3];
            cursor.read_exact(&mut start_code_prefix).hand_log(|msg| warn!("{msg}"))?;
            let stream_id = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
            match stream_id {
                PS_PES_START_CODE_VIDEO_FIRST..=PS_PES_START_CODE_VIDEO_LAST => {
                    if let Some(pes_pkt) = Self::read_video_pes_data(&mut cursor, stream_id).hand_log(|msg| warn!("{msg}"))? {
                        pes_packets.push(pes_pkt);
                    }
                }
                PS_PES_START_CODE_AUDIO_FIRST..=PS_PES_START_CODE_AUDIO_LAST => {
                    Self::read_audio_pes_data(&mut cursor).hand_log(|msg| warn!("{msg}"))?;
                }
                _ => {
                    return Err(GlobalError::new_sys_error(&format!("invalid data:start_code+ident = 0x000001{:02x}", stream_id), |msg| warn!("{msg}")));
                }
            }
        }
        Ok(pes_packets)
    }

    //audio 暂不支持读取内容，仅做字节跳过,
    fn read_audio_pes_data(cursor: &mut Cursor<Bytes>) -> GlobalResult<()> {
        let packet_len = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let remain_len = if packet_len == 0 { Self::get_pkt_len(cursor) } else { packet_len as usize };
        cursor.seek(SeekFrom::Current(remain_len as i64)).hand_log(|msg| info!("pes packet len greater data len:{msg}"))?;
        Ok(())
    }

    fn read_video_pes_data(cursor: &mut Cursor<Bytes>, stream_id: u8) -> GlobalResult<Option<Self>> {
        let packet_len = cursor.read_u16::<BigEndian>().hand_log(|msg| warn!("{msg}"))?;
        let remain_len = if packet_len == 0 { Self::get_pkt_len(cursor) } else { packet_len as usize };
        if Self::check_stream_id_pts_dts_info(stream_id) {
            let m2_p2_p1_d1_c1_o1 = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
            let flags2_e1_e1_d1_a1_p1_p1 = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
            let header_len = cursor.read_u8().hand_log(|msg| warn!("{msg}"))?;
            let mut mut_header_main_info = BytesMut::with_capacity(header_len as usize);
            unsafe { mut_header_main_info.set_len(header_len as usize); }
            cursor.read_exact(&mut *mut_header_main_info).hand_log(|msg| warn!("{msg}"))?;

            let payload_len = remain_len - header_len as usize - 3; //去掉2个字节flag + 1 header_len
            let mut mut_pes_payload = BytesMut::with_capacity(payload_len);
            unsafe { mut_pes_payload.set_len(payload_len); }
            cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| {
                warn!("{msg}");
            })?;

            let pts_dts_info = PesPtsDtsInfo {
                m2_p2_p1_d1_c1_o1,
                flags2_e1_e1_d1_a1_p1_p1,
                header_len,
                header_main_info: mut_header_main_info.freeze(),
                pes_payload: mut_pes_payload.freeze(),
            };
            let pes_packet = Self {
                start_code_prefix: PS_START_CODE_PREFIX,
                stream_id,
                packet_len,
                pes_inner_data: PesInnerData::PesPtsDtsInfo(pts_dts_info),
            };
            return Ok(Some(pes_packet));
        } else if Self::check_stream_id_pes_packet_data(stream_id) {
            let mut mut_pes_payload = BytesMut::with_capacity(remain_len);
            unsafe { mut_pes_payload.set_len(remain_len); }
            cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| warn!("{msg}"))?;

            let pes_packet = Self {
                start_code_prefix: PS_START_CODE_PREFIX,
                stream_id,
                packet_len,
                pes_inner_data: PesInnerData::PesAllData(mut_pes_payload.freeze()),
            };
            return Ok(Some(pes_packet));
        } else if Self::check_stream_id_padding(stream_id) {
            let mut mut_pes_payload = BytesMut::with_capacity(remain_len);
            unsafe { mut_pes_payload.set_len(remain_len); }
            cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| warn!("{msg}"))?;

            let pes_packet = Self {
                start_code_prefix: PS_START_CODE_PREFIX,
                stream_id,
                packet_len,
                pes_inner_data: PesInnerData::PesAllPadding(mut_pes_payload.freeze()),
            };
            return Ok(Some(pes_packet));
        }
        Ok(None)
    }

    //PES_packet_length == 0|0xFFFF,读取数据直到下一个PES包头0x000001+ident,或到数据流的结束
    fn get_pkt_len(cursor: &Cursor<Bytes>) -> usize {
        let bytes = cursor.get_ref();
        let pos = cursor.position() as usize;
        let mut iter = memmem::find_iter(&bytes[pos..], &PS_START_CODE_PREFIX);
        while let Some(index) = iter.next() {
            let i = pos + index + 3;
            if matches!(bytes[i],PS_PES_START_CODE_VIDEO_FIRST..=PS_PES_START_CODE_VIDEO_LAST
                        | PS_PES_START_CODE_AUDIO_FIRST..=PS_PES_START_CODE_AUDIO_LAST)
            {
                return index;
            }
        }
        bytes.len() - pos
    }

    fn check_stream_id_pts_dts_info(stream_id: u8) -> bool {
        stream_id != 0b1011_1100 // program_stream_map
            && stream_id != 0b1011_1110 // padding_stream
            && stream_id != 0b1011_1111 // private_stream-2
            && stream_id != 0b1111_0000 // ECM_stream
            && stream_id != 0b1111_0001 // EMM_stream
            && stream_id != 0b1111_1111 // program_stream_directory
    }
    fn check_stream_id_pes_packet_data(stream_id: u8) -> bool {
        matches!(stream_id,0b1011_1100|0b1011_1111|0b1111_0000|0b1111_0001|0b1111_1111)
    }
    fn check_stream_id_padding(stream_id: u8) -> bool {
        stream_id == 0b1011_1110
    }
}

pub enum PesInnerData {
    PesPtsDtsInfo(PesPtsDtsInfo),
    PesAllData(Bytes),
    PesAllPadding(Bytes),
}

#[allow(dead_code)]
pub struct PesPtsDtsInfo {
    m2_p2_p1_d1_c1_o1: u8,
    flags2_e1_e1_d1_a1_p1_p1: u8,
    header_len: u8,
    header_main_info: Bytes,
    pes_payload: Bytes,
}

#[cfg(test)]
mod test {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use byteorder::ReadBytesExt;
    use memchr::memmem;

    use common::bytes::{Bytes, BytesMut};

    use crate::container::ps::{PsHeader, PsPacket, PsSysHeader, PsSysMap};

    #[test]
    fn test_parse_ps_header() {
        let data = [00u8, 0x00u8, 0x01u8, 0xbau8, 0x44u8, 0xf0u8, 0x4fu8, 0x69u8, 0x64u8, 0x01u8, 0x02u8, 0x5fu8, 0x03u8, 0xfeu8, 0xffu8, 0xffu8, 0x00u8, 0x01u8, 0x11u8, 0x0cu8];
        let mut cursor = Cursor::new(Bytes::from(data.to_vec()));
        let ps_header_res = PsHeader::parse(&mut cursor);
        assert!(ps_header_res.is_ok());
        let ps_header = ps_header_res.unwrap();
        assert_eq!(ps_header.stuffing_byte.last(), Some(&0x0cu8));
        assert_eq!(cursor.position(), 20);
    }

    #[test]
    fn test_parse_ps_sys_header() {
        let data = [00u8, 0x00u8, 0x01u8, 0xBBu8, 0x00u8, 0x09u8, 0x81u8, 0x86u8, 0xA1u8, 0x05u8, 0xE1u8, 0x7Eu8, 0xE0u8, 0xE8u8, 0x00u8];
        let mut cursor = Cursor::new(Bytes::from(data.to_vec()));
        let ps_sys_header_res = PsSysHeader::parse(&mut cursor);
        assert!(ps_sys_header_res.is_ok());
        let ps_sys_header = ps_sys_header_res.unwrap();
        assert_eq!(ps_sys_header.ps_stream_vec.len(), 1);
        assert_eq!(ps_sys_header.ps_stream_vec.get(0).unwrap().stream_id, 0xe0);
        assert_eq!(ps_sys_header.ps_stream_vec.get(0).unwrap().p_psd, [0xE8u8, 0x00u8]);

        let data = [00u8, 0x00u8, 0x01u8, 0xbbu8, 0x00u8, 0x12u8, 0x81u8, 0x2fu8, 0x81u8, 0x04u8, 0xe1u8, 0x7fu8, 0xe0u8, 0xe0u8, 0x80u8, 0xc0u8, 0xc0u8, 0x08u8, 0xbdu8, 0xe0u8, 0x80u8, 0xbfu8, 0xe0u8, 0x80];
        let mut cursor = Cursor::new(Bytes::from(data.to_vec()));
        let ps_sys_header_res = PsSysHeader::parse(&mut cursor);
        println!("{:?}", ps_sys_header_res);
        assert!(ps_sys_header_res.is_ok());
    }

    #[test]
    fn test_parse_ps_sys_map() {
        // let data = [0x00u8, 0x00u8, 0x01u8, 0xbcu8, 0x00u8, 0x5eu8, 0xf8u8, 0xffu8, 0x00u8, 0x24u8,
        //     0x40u8, 0x0eu8, 0x48u8, 0x4bu8, 0x01u8, 0x00u8, 0x14u8, 0x14u8, 0x40u8, 0x16u8, 0x6bu8, 0xbfu8, 0x00u8,
        //     0xffu8, 0xffu8, 0xffu8, 0x41u8, 0x12u8, 0x48u8, 0x4bu8, 0x00u8, 0x01u8, 0x02u8, 0x03u8, 0x04u8, 0x05u8,
        //     0x06u8, 0x07u8, 0x08u8, 0x09u8, 0x0au8, 0x0bu8, 0x0cu8, 0x0du8, 0x0eu8, 0x0fu8, 0x00u8, 0x30u8, 0x1bu8,
        //     0xe0u8, 0x00u8, 0x1cu8, 0x42u8, 0x0eu8, 0x07u8, 0x10u8, 0x10u8, 0xeau8, 0x05u8, 0x00u8, 0x02u8, 0xd0u8,
        //     0x11u8, 0x30u8, 0x00u8, 0x00u8, 0x1cu8, 0x21u8, 0x2au8, 0x0au8, 0x7fu8, 0xffu8, 0x00u8, 0x00u8, 0x07u8,
        //     0x08u8, 0x1fu8, 0xfeu8, 0xa0u8, 0x5au8, 0x90u8, 0xc0u8, 0x00u8, 0x0cu8, 0x43u8, 0x0au8, 0x01u8, 0x40u8,
        //     0xfeu8, 0x00u8, 0x7du8, 0x03u8, 0x03u8, 0xe8u8, 0x03u8, 0xffu8, 0xf6u8, 0x53u8, 0x94u8, 0x03u8];
        // let mut cursor = Cursor::new(Bytes::from(data.to_vec()));
        // let ps_sys_map_res = PsSysMap::parse(&mut cursor);
        // println!("{:0x.hand_log(|msg|warn!("{msg}"))?}", ps_sys_map_res);
        // assert_eq!(cursor.position(), 100);


        let data1 = [0x00u8, 0x00, 0x01, 0xbc, 0x00, 0x3f, 0xc2, 0x01, 0x00, 0x00, 0x00, 0x35,
            0x1b, 0xe0, 0x00, 0x28, 0x01, 0x42, 0xc0, 0x1e, 0xff, 0xe1, 0x00, 0x18, 0x67, 0x42, 0xc0,
            0x1e, 0xda, 0x01, 0xe0, 0x08, 0x9f, 0x96, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0x20, 0xf1, 0x62, 0xea, 0x01, 0x00, 0x05, 0x68, 0xce, 0x0f, 0x2c, 0x80, 0x0f,
            0xc0, 0x00, 0x05, 0x11, 0x90, 0x56, 0xe5, 0x00, 0x1e, 0xb3, 0x9f, 0x92, 0x00u8, 0x00, 0x01, 0xe0];
        let mut cursor = Cursor::new(Bytes::from(data1.to_vec()));
        let ps_sys_map_res = PsSysMap::parse(&mut cursor);
        println!("{:?}", ps_sys_map_res);
        // assert_eq!(cursor.position(), 100);
    }

    #[test]
    fn test_ps_parse() {
        let input = include_bytes!("/mnt/e/code/rust/study/media/rsmpeg/tests/assets/vids/ps.raw");
        let bytes = Bytes::copy_from_slice(input);
        let mut ps_packet = PsPacket::default();
        if let Ok(Some(mut vec)) = ps_packet.parse(true, bytes) {
            println!("len_1 = {}", vec.len());
            vec.iter().map(|iter| println!("data len = {}", iter.len())).count();
            // vec.iter().map(|iter| println!("data = {:02x?}", iter.to_vec())).count();
        }
    }


    #[test]
    fn test_cursor_position() {
        let data = [0x00u8, 0x00, 0x01, 0xbc, 0x00, 0x3f, 0xc2, 0x01, 0x00, 0x00, 0x00, 0x35,
            0x1b, 0xe0, 0x00, 0x28, 0x01, 0x42, 0xc0, 0x1e, 0xff, 0xe1, 0x00, 0x18, 0x67, 0x42, 0xc0,
            0x1e, 0xda, 0x01, 0xe0, 0x08, 0x9f, 0x96, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0x20, 0xf1, 0x62, 0xea, 0x01, 0x00, 0x05, 0x68, 0xce, 0x0f, 0x2c, 0x80, 0x0f,
            0xc0, 0x00, 0x05, 0x11, 0x90, 0x56, 0xe5, 0x00, 0x1e, 0xb3, 0x9f, 0x92, 0x00u8, 0x00, 0x01, 0xe0];
        let mut cursor = Cursor::new(data);
        let packet_len = 10;
        let mut mut_pes_payload = BytesMut::with_capacity(packet_len);
        unsafe { mut_pes_payload.set_len(packet_len); }
        let result = cursor.read_exact(&mut *mut_pes_payload);
        println!("{:02x?}", mut_pes_payload.to_vec());
        println!("position = {}", cursor.position());
    }

    #[test]
    fn test_mem_find() {
        let data = Bytes::from_static(&[0x12, 0x34, 0x00, 0x00, 0x01, 0xAB, 0xCD]);
        let mut cursor = Cursor::new(data);
        let first = cursor.read_u8().unwrap();
        let bytes = cursor.get_ref();
        println!("{:02?}", bytes.to_vec());
        let pos = cursor.position() as usize;
        let sequence = [0x00, 0x00, 0x01];
        if let Some(mut index) = memmem::find(&bytes[pos..], &sequence) {
            index += pos;
            println!("index = {},val = {:02x}", index, bytes[index]);
        }
        cursor.seek(SeekFrom::End(0)).unwrap();
        println!("{}", cursor.position());
    }
}