use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use byteorder::{BigEndian, ReadBytesExt};

use common::anyhow::anyhow;
use common::bytes::{Buf, BufMut};
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::info;
use crate::coder::h264::H264;

const PS_PACK_START_CODE: u32 = 0x000001BA;
const PS_SYS_START_CODE: u32 = 0x000001BB;
const PS_SYS_MAP_START_CODE: u32 = 0x000001BC;
const PES_HEADER_START_CODE_VIDEO: u32 = 0x000001E0;
const PES_HEADER_START_CODE_AUDIO: u32 = 0x000001C0;

#[derive(Default)]
pub struct PsPacket {
    ps_header: Option<PsHeader>,
    ps_sys_header: Option<PsSysHeader>,
    ps_sys_map: Option<PsSysMap>,
    pes_data: BytesMut,
}

impl PsPacket {
    pub fn parse(&mut self, bytes: Bytes) -> GlobalResult<Option<Vec<Bytes>>> {
        if let Some(pes_data) = self.read_to_payload(bytes)? {
            let vec = self.parse_to_nalu(pes_data)?;
            return Ok(Some(vec));
        }
        Ok(None)
    }

    fn read_to_payload(&mut self, bytes: Bytes) -> GlobalResult<Option<Bytes>> {
        let bytes_len = bytes.len();
        if bytes_len < PsHeader::PS_HEADER_BASE_LEN as usize {
            self.pes_data.put(bytes);
            Ok(None)
        } else {
            let start_code = bytes.slice(0..4).get_u32();
            match start_code {
                PS_PACK_START_CODE => {
                    let mut res = None;
                    if self.pes_data.len() > 0 {
                        let pes_data = std::mem::take(&mut self.pes_data);
                        res = Some(pes_data.freeze());
                    }
                    let mut cursor = Cursor::new(bytes);
                    let ps_header = PsHeader::parse(&mut cursor)?;
                    self.ps_header = Some(ps_header);
                    let start_code = cursor.read_u32::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
                    cursor.seek(SeekFrom::Current(-4)).hand_log(|msg| info!("{msg}"))?;
                    if start_code == PS_SYS_START_CODE {
                        let ps_sys_header = PsSysHeader::parse(&mut cursor).hand_log(|msg| info!("{msg}"))?;
                        let ps_sys_map = PsSysMap::parse(&mut cursor).hand_log(|msg| info!("{msg}"))?;
                        self.ps_sys_header = Some(ps_sys_header);
                        self.ps_sys_map = Some(ps_sys_map);
                    }
                    let pes_buf_len = bytes_len - cursor.position() as usize;
                    let mut pes_buf_data = BytesMut::with_capacity(pes_buf_len);
                    unsafe { pes_buf_data.set_len(pes_buf_len); }
                    cursor.read_exact(&mut *pes_buf_data).hand_log(|msg| info!("{msg}"))?;
                    self.pes_data.put(pes_buf_data);
                    Ok(res)
                }
                _ => {
                    self.pes_data.put(bytes);
                    Ok(None)
                }
            }
        }
    }
    fn parse_to_nalu(&self, bytes: Bytes) -> GlobalResult<Vec<Bytes>> {
        let mut nalus = Vec::new();
        if let Some(sys_map) = &self.ps_sys_map {
            let pes_packets = PesPacket::read_es_data(bytes)?;
            for pes_packet in pes_packets {
                let stream_type = &sys_map.es_map_info.get(&pes_packet.stream_id).ok_or_else(|| SysErr(anyhow!("stream id in es not found in ps sys map.")))?.stream_type;
                match stream_type {
                    //H264
                    &0x1B => {
                        match pes_packet.pes_inner_data {
                            PesInnerData::PesPtsDtsInfo(PesPtsDtsInfo { pes_payload, .. }) => {
                                let nals = H264::extract_nal_by_annexb(pes_payload);
                                nalus.extend(nals);
                            }
                            PesInnerData::PesAllData(pes_payload) => {
                                let nals = H264::extract_nal_by_annexb(pes_payload);
                                nalus.extend(nals);
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
                        return Err(SysErr(anyhow!("系统暂不支持类型：{stream_type}")));
                    }
                };
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
pub struct PsHeader {
    start_code: u32,
    ver_system_clock_reference_base_marker: [u8; 6],
    program_mux_rate22_marker_bit1_x2: [u8; 3],
    reserved5_pack_stuffing_length3: u8,
    stuffing_byte: Bytes,
}

impl PsHeader {
    const PS_HEADER_BASE_LEN: u8 = 14;
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let start_code = cursor.read_u32::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        // if start_code != PS_PACK_START_CODE {
        //     return Err(GlobalError::new_biz_error(0, "invalid ps_pack_start_code", |msg| debug!("{msg}")));
        // }
        //SCR-System Clock Reference = 6 bytes
        let mut ver_system_clock_reference_base_marker = [0u8; 6];
        cursor.read_exact(&mut ver_system_clock_reference_base_marker).hand_log(|msg| info!("{msg}"))?;
        //program_mux_rate = 3 bytes
        let mut program_mux_rate22_marker_bit1_x2 = [0u8; 3];
        cursor.read_exact(&mut program_mux_rate22_marker_bit1_x2).hand_log(|msg| info!("{msg}"))?;
        let reserved5_pack_stuffing_length3 = cursor.read_u8().hand_log(|msg| info!("{msg}"))? & 0b0000_0111u8;
        let mut mut_stuffing_byte = BytesMut::with_capacity(reserved5_pack_stuffing_length3 as usize);
        // mut_stuffing_byte.resize(reserved5_pack_stuffing_length3 as usize, 0);
        unsafe { mut_stuffing_byte.set_len(reserved5_pack_stuffing_length3 as usize); }
        if reserved5_pack_stuffing_length3 > 0 {
            cursor.read_exact(&mut *mut_stuffing_byte).hand_log(|msg| info!("{msg}"))?;
        }
        let ps_header = Self {
            start_code,
            ver_system_clock_reference_base_marker,
            program_mux_rate22_marker_bit1_x2,
            reserved5_pack_stuffing_length3,
            stuffing_byte: mut_stuffing_byte.freeze(),
        };
        Ok(ps_header)
    }
}

#[derive(Debug)]
pub struct PsSysHeader {
    start_code: u32,
    len: u16,
    rate_audio_video_band_flag: [u8; 6],
    ps_stream_vec: Vec<PsStream>,
}

impl PsSysHeader {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let start_code = cursor.read_u32::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        // if start_code != PS_SYS_START_CODE {
        //     return Err(SysErr(anyhow!("invalid ps_sys_start_code")));
        // }
        let len = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        let index = cursor.position() + len as u64;
        let mut rate_audio_video_band_flag = [0u8; 6];
        cursor.read_exact(&mut rate_audio_video_band_flag).hand_log(|msg| info!("{msg}"))?;
        let mut ps_stream_vec = Vec::new();
        while cursor.position() < index {
            let stream_id = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
            if stream_id >> 7 == 1 {
                let mut p_psd = [0u8; 2];
                cursor.read_exact(&mut p_psd).hand_log(|msg| info!("{msg}"))?;
                let ps_stream = PsStream { stream_id, p_psd };
                ps_stream_vec.push(ps_stream);
            } else {
                break;
            }
        }
        cursor.seek(SeekFrom::Start(index)).hand_log(|msg| info!("{msg}"))?;
        Ok(Self {
            start_code,
            len,
            rate_audio_video_band_flag,
            ps_stream_vec,
        })
    }
}

#[derive(Debug)]
pub struct PsStream {
    stream_id: u8,
    p_psd: [u8; 2],
}

#[derive(Debug)]
pub struct PsSysMap {
    start_code3_map_stream_id8: u32,
    ps_map_length: u16,
    indicator1_reserved2_version5: u8,
    reserved7_marker1: u8,
    ps_info_length: u16,
    ps_info_descriptor: Vec<Descriptor>,
    es_map_length: u16,
    es_map_info: HashMap<u8, EsInfo>,
    crc_32: u32,
}

impl PsSysMap {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let start_code3_map_stream_id8 = cursor.read_u32::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        if start_code3_map_stream_id8 != PS_SYS_MAP_START_CODE {
            return Err(SysErr(anyhow!("invalid ps_sys_map_start_code")));
        }
        let ps_map_length = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        let indicator1_reserved2_version5 = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let reserved7_marker1 = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let ps_info_length = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        let ps_info_index = cursor.position() + ps_info_length as u64;
        let mut ps_info_descriptor = Vec::new();
        while cursor.position() < ps_info_index {
            let descriptor = Descriptor::parse(cursor)?;
            ps_info_descriptor.push(descriptor);
        }
        let es_map_length = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        let es_map_index = cursor.position() + es_map_length as u64;
        let mut es_map_info = HashMap::new();
        while cursor.position() < es_map_index {
            let (es_id, es_info) = EsInfo::parse(cursor)?;
            es_map_info.insert(es_id, es_info);
        }
        let crc_32 = cursor.read_u32::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
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
pub struct EsInfo {
    stream_type: u8,
    es_info_length: u16,
    es_info_descriptor: Vec<Descriptor>,
}

impl EsInfo {
    //stream_id:EsInfo
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<(u8, Self)> {
        let stream_type = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let es_id = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let es_info_length = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
        let es_info_index = cursor.position() + es_info_length as u64;
        let mut es_info_descriptor = Vec::new();
        while cursor.position() < es_info_index {
            let descriptor = Descriptor::parse(cursor)?;
            es_info_descriptor.push(descriptor);
        }
        Ok((es_id, Self {
            stream_type,
            es_info_length,
            es_info_descriptor,
        }))
    }
}


#[derive(Debug)]
pub struct Descriptor {
    descriptor_tag: u8,
    descriptor_length: u8,
    descriptor_data: Bytes,
}

impl Descriptor {
    pub fn parse(cursor: &mut Cursor<Bytes>) -> GlobalResult<Self> {
        let descriptor_tag = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let descriptor_length = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
        let mut descriptor_data = BytesMut::with_capacity(descriptor_length as usize);
        unsafe { descriptor_data.set_len(descriptor_length as usize); }
        cursor.read_exact(&mut *descriptor_data).hand_log(|msg| info!("{msg}"))?;
        Ok(Descriptor {
            descriptor_tag,
            descriptor_length,
            descriptor_data: descriptor_data.freeze(),
        })
    }
}

pub struct PesPacket {
    start_code_prefix: [u8; 3],
    stream_id: u8,
    packet_len: u16,
    pes_inner_data: PesInnerData,
}

impl PesPacket {
    const PES_START_CODE_PREFIX: [u8; 3] = [0x00, 0x00, 0x01u8];
    pub fn read_es_data(bytes: Bytes) -> GlobalResult<Vec<Self>> {
        let data_len = bytes.len();
        let mut cursor = Cursor::new(bytes);
        let mut pes_packets = Vec::new();
        while cursor.position() < data_len as u64 {
            let mut start_code_prefix = [0u8; 3];
            cursor.read_exact(&mut start_code_prefix).hand_log(|msg| info!("{msg}"))?;
            let stream_id = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
            if start_code_prefix != Self::PES_START_CODE_PREFIX || !matches!(stream_id,(0xC0..=0xDF)|(0xE0..=0xEF)) {
                return Err(SysErr(anyhow!("pes:invalid data")));
            }
            let packet_len = cursor.read_u16::<BigEndian>().hand_log(|msg| info!("{msg}"))?;
            if Self::check_stream_id_pts_dts_info(stream_id) {
                let m2_p2_p1_d1_c1_o1 = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
                let flags2_e1_e1_d1_a1_p1_p1 = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
                let header_len = cursor.read_u8().hand_log(|msg| info!("{msg}"))?;
                let mut mut_header_main_info = BytesMut::with_capacity(header_len as usize);
                unsafe { mut_header_main_info.set_len(header_len as usize); }
                cursor.read_exact(&mut *mut_header_main_info).hand_log(|msg| info!("{msg}"))?;

                let payload_len = packet_len as usize - header_len as usize - 2;
                let mut mut_pes_payload = BytesMut::with_capacity(payload_len);
                unsafe { mut_pes_payload.set_len(payload_len); }
                cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| info!("{msg}"))?;

                let pts_dts_info = PesPtsDtsInfo {
                    m2_p2_p1_d1_c1_o1,
                    flags2_e1_e1_d1_a1_p1_p1,
                    header_len,
                    header_main_info: mut_header_main_info.freeze(),
                    pes_payload: mut_pes_payload.freeze(),
                };
                let pes_packet = Self {
                    start_code_prefix,
                    stream_id,
                    packet_len,
                    pes_inner_data: PesInnerData::PesPtsDtsInfo(pts_dts_info),
                };
                pes_packets.push(pes_packet);
            } else if Self::check_stream_id_pes_packet_data(stream_id) {
                let mut mut_pes_payload = BytesMut::with_capacity(packet_len as usize);
                unsafe { mut_pes_payload.set_len(packet_len as usize); }
                cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| info!("{msg}"))?;

                let pes_packet = Self {
                    start_code_prefix,
                    stream_id,
                    packet_len,
                    pes_inner_data: PesInnerData::PesAllData(mut_pes_payload.freeze()),
                };
                pes_packets.push(pes_packet);
            } else if Self::check_stream_id_padding(stream_id) {
                let mut mut_pes_payload = BytesMut::with_capacity(packet_len as usize);
                unsafe { mut_pes_payload.set_len(packet_len as usize); }
                cursor.read_exact(&mut *mut_pes_payload).hand_log(|msg| info!("{msg}"))?;

                let pes_packet = Self {
                    start_code_prefix,
                    stream_id,
                    packet_len,
                    pes_inner_data: PesInnerData::PesAllPadding(mut_pes_payload.freeze()),
                };
                pes_packets.push(pes_packet);
            }
        }
        Ok(pes_packets)
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

pub struct PesPtsDtsInfo {
    m2_p2_p1_d1_c1_o1: u8,
    flags2_e1_e1_d1_a1_p1_p1: u8,
    header_len: u8,
    header_main_info: Bytes,
    pes_payload: Bytes,
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use common::bytes::Bytes;

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
        println!("{:0x?}", ps_sys_header_res);
        assert!(ps_sys_header_res.is_ok());
    }

    #[test]
    fn test_parse_ps_sys_map() {
        let data = [0x00u8, 0x00u8, 0x01u8, 0xbcu8, 0x00u8, 0x5eu8, 0xf8u8, 0xffu8, 0x00u8, 0x24u8,
            0x40u8, 0x0eu8, 0x48u8, 0x4bu8, 0x01u8, 0x00u8, 0x14u8, 0x14u8, 0x40u8, 0x16u8, 0x6bu8, 0xbfu8, 0x00u8,
            0xffu8, 0xffu8, 0xffu8, 0x41u8, 0x12u8, 0x48u8, 0x4bu8, 0x00u8, 0x01u8, 0x02u8, 0x03u8, 0x04u8, 0x05u8,
            0x06u8, 0x07u8, 0x08u8, 0x09u8, 0x0au8, 0x0bu8, 0x0cu8, 0x0du8, 0x0eu8, 0x0fu8, 0x00u8, 0x30u8, 0x1bu8,
            0xe0u8, 0x00u8, 0x1cu8, 0x42u8, 0x0eu8, 0x07u8, 0x10u8, 0x10u8, 0xeau8, 0x05u8, 0x00u8, 0x02u8, 0xd0u8,
            0x11u8, 0x30u8, 0x00u8, 0x00u8, 0x1cu8, 0x21u8, 0x2au8, 0x0au8, 0x7fu8, 0xffu8, 0x00u8, 0x00u8, 0x07u8,
            0x08u8, 0x1fu8, 0xfeu8, 0xa0u8, 0x5au8, 0x90u8, 0xc0u8, 0x00u8, 0x0cu8, 0x43u8, 0x0au8, 0x01u8, 0x40u8,
            0xfeu8, 0x00u8, 0x7du8, 0x03u8, 0x03u8, 0xe8u8, 0x03u8, 0xffu8, 0xf6u8, 0x53u8, 0x94u8, 0x03u8];
        let mut cursor = Cursor::new(Bytes::from(data.to_vec()));
        let ps_sys_map_res = PsSysMap::parse(&mut cursor);
        println!("{:0x?}", ps_sys_map_res);
        assert_eq!(cursor.position(), 100);
    }

    #[test]
    fn test_ps_parse() {
        // let input = include_bytes!("/mnt/e/code/rust/study/media/rsmpeg/tests/assets/vids/ps.raw");
        // let mut ps_packet = PsPacket::default();
        // if let Ok(Some(bytes)) = ps_packet.read_to_payload(Bytes::copy_from_slice(input)) {
        //     if let Ok(vec) = ps_packet.parse_to_nalu(bytes) {
        //         vec.iter().map(|iter| println!("{:02x?}", iter.to_vec())).count();
        //     }
        // }
    }
}