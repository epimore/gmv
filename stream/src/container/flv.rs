use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::GlobalResult;
use constructor::New;

use crate::coder::h264::ANNEXB_NALUSTART_CODE;
use crate::container::HandleMuxerDataFn;

pub struct FlvHeader {
    signature: [u8; 3], // "FLV"
    version: u8,
    flags: u8,
    header_length: u32,
}

impl FlvHeader {
    pub fn get_header_byte_and_previos_tag_size0(video: bool, audio: bool) -> (Bytes, Bytes) {
        let tag_bytes = FlvHeader::build(video, audio).to_bytes();
        let previos_tag_size = Bytes::from(0u32.to_be_bytes().to_vec());
        (tag_bytes, previos_tag_size)
    }

    pub fn process(f: HandleMuxerDataFn, video: bool, audio: bool) -> GlobalResult<()> {
        let tag_bytes = FlvHeader::build(video, audio).to_bytes();
        let previos_tag_size = Bytes::from(0u32.to_be_bytes().to_vec());
        f(tag_bytes)?;
        f(previos_tag_size)
    }
    fn build(video: bool, audio: bool) -> Self {
        let flags = if video && audio {
            0b00000101u8
        } else if audio {
            0b00000100u8
        } else if video {
            0b00000001u8
        } else {
            panic!("Flv Header Flags must a have media type");
        };
        Self {
            signature: *b"FLV",
            version: 0x01,
            flags,
            header_length: 9,
        }
    }
    fn to_bytes(self) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put(&self.signature[..]);
        bm.put_u8(self.version);
        bm.put_u8(self.flags);
        bm.put_u32(self.header_length);
        bm.freeze()
    }
}

pub struct FlvTag {
    tag_type: u8,
    data_size: [u8; 3],
    timestamp: [u8; 3],
    timestamp_ext: u8,
    stream_id: u32,
    data: Bytes,
}

impl FlvTag {
    pub fn process(f: HandleMuxerDataFn, tag_type: TagType, ts: u32, data: Bytes) -> GlobalResult<()> {
        let tag_bytes = FlvTag::build(tag_type, ts, data).to_bytes(0x17, 1);
        let len_vec = (tag_bytes.len() as u32).to_be_bytes().to_vec();
        let previos_tag_size = Bytes::from(len_vec);
        f(tag_bytes)?;
        f(previos_tag_size)
    }

    fn build(tag_type: TagType, ts: u32, data: Bytes) -> Self {
        let data_arr = (data.len() as u32).to_be_bytes();
        let data_size = [data_arr[1], data_arr[2], data_arr[3]];
        let ts_arr = ts.to_be_bytes();
        Self {
            tag_type: tag_type.get_value(),
            data_size,
            timestamp: [ts_arr[1], ts_arr[2], ts_arr[3]],
            timestamp_ext: ts_arr[0],
            stream_id: 0,
            data,
        }
    }
    //ft_ci:FrameType 4bit/CodecID 4bit
    fn to_bytes(self, ft_ci: u8, avc_packet_type: u8) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put_u8(self.tag_type);
        bm.put_slice(&self.data_size);
        bm.put_slice(&self.timestamp);
        bm.put_u8(self.timestamp_ext);
        bm.put_slice(&[0x00, 0x00, 0x00]);
        bm.put_u8(avc_packet_type); //AVCPacketType
        bm.put_slice(&[0u8, 0, 0]); //CompositionTime Offset
        bm.put_u8(ft_ci);
        bm.put(&self.data[..]);
        bm.freeze()
    }
}

pub enum TagType {
    Audio,
    Video,
    Script,
}

impl TagType {
    fn get_value(self) -> u8 {
        match self {
            TagType::Audio => { 8 }
            TagType::Video => { 9 }
            TagType::Script => { 18 }
        }
    }
}

#[derive(New)]
pub struct AVCDecoderConfiguration {
    sps: Bytes,
    pps: Bytes,
    ts: u32,
}

impl AVCDecoderConfiguration {
    pub fn to_flv_tag_bytes(self) -> Bytes {
        let sps = self.sps;
        let pps = self.pps;
        let mut video_tag_data = BytesMut::new();
        // FLV video tag data start
        // video_tag_data.put_u8(0x17); // FrameType (key frame) + CodecID (AVC)
        // video_tag_data.put_u8(0x00); // AVC packet type (sequence header)
        // video_tag_data.put_slice(&[0u8, 0, 0]); // CompositionTime

        // AVCDecoderConfigurationRecord
        video_tag_data.put_u8(0x01); // ConfigurationVersion
        video_tag_data.put_u8(sps[1]); // AVCProfileIndication
        video_tag_data.put_u8(sps[2]); // ProfileCompatibility
        video_tag_data.put_u8(sps[3]); // AVCLevelIndication
        video_tag_data.put_u8(0xff); // Reserved + lengthSizeMinusOne

        video_tag_data.put_u8(0xe1); // Reserved + numOfSequenceParameterSets
        video_tag_data.put_u16(sps.len() as u16); // SPS length
        video_tag_data.put(sps); // SPS NALU

        video_tag_data.put_u8(0x01); // numOfPictureParameterSets
        video_tag_data.put_u16(pps.len() as u16); // PPS length
        video_tag_data.put(pps); // PPS NALU

        FlvTag::build(TagType::Video, self.ts, video_tag_data.freeze()).to_bytes(0x17, 0)
    }
}