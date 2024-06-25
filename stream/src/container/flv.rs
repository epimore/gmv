use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::GlobalResult;
use constructor::New;

use crate::container::HandleMuxerDataFn;

const FLV_MTU: usize = 1200;

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
        for chunk in data.chunks(FLV_MTU) {
            let sub_tag_bytes = FlvTag::build(tag_type, ts, Bytes::copy_from_slice(chunk)).to_bytes(0x17, 1);
            let len_vec = (sub_tag_bytes.len() as u32).to_be_bytes().to_vec();
            let previos_tag_size = Bytes::from(len_vec);
            f(sub_tag_bytes)?;
            f(previos_tag_size)?;
        }
        Ok(())
    }

    fn build(tag_type: TagType, ts: u32, data: Bytes) -> Self {
        let size_arr = (data.len() as u32).to_be_bytes();
        let data_size = [size_arr[1], size_arr[2], size_arr[3]];
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
        bm.put_u8(ft_ci);
        bm.put_u8(avc_packet_type); //AVCPacketType
        bm.put_slice(&[0u8, 0, 0]); //CompositionTime Offset
        bm.put(&self.data[..]);
        bm.freeze()
    }
}

#[derive(Copy, Clone)]
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

pub struct ScriptTag {
    duration: f64,
    width: u32,
    height: u32,
    videodatarate: u32,
    framerate: u32,
    videocodecid: u32,
    audiodatarate: u32,
    audiosamplerate: u32,
    audiosamplesize: u32,
    stereo: bool,
    audiocodecid: u32,
    filesize: u64,
}

impl ScriptTag {
    pub fn build_script_tag_bytes(tag_data: Bytes) -> Bytes {
        let data_size = tag_data.len() as u32;
        let size_arr = data_size.to_be_bytes();
        let data_size_slice = [size_arr[1], size_arr[2], size_arr[3]];
        let mut packet = BytesMut::new();
        packet.put_u8(0x12); // script tag
        packet.put_slice(&data_size_slice); // data size
        packet.put_slice(&[0u8,0,0]); // timestamp
        packet.put_u32(0); // stream ID
        packet.put(tag_data);
        // PreviousTagSize
        packet.put_u32(11 + data_size);
        packet.freeze()
    }

    pub fn build_script_tag_data(width: u32, height: u32, framerate: f64) -> Bytes {
        let mut tag_data = BytesMut::new();

        // ECMA array with metadata
        tag_data.put_u8(0x02); // type: string
        tag_data.put_u16(0x0A); // length: 10
        tag_data.put_slice(b"onMetaData"); // string: "onMetaData"
        tag_data.put_u8(0x08); // type: ECMA array
        tag_data.put_u32(0x00_00_00_0A); // number of elements: 10
        tag_data.put_slice(&Self::build_amf_string("duration", 0.0)); // duration
        tag_data.put_slice(&Self::build_amf_string("width", width as f64)); // width
        tag_data.put_slice(&Self::build_amf_string("height", height as f64)); // height
        tag_data.put_slice(&Self::build_amf_string("videodatarate", 5000.0)); // videodatarate
        tag_data.put_slice(&Self::build_amf_string("framerate", framerate)); // framerate
        tag_data.put_slice(&Self::build_amf_string("videocodecid", 7.0)); // videocodecid (7 for AVC)
        tag_data.put_slice(&Self::build_amf_string("audiodatarate", 128.0)); // audiodatarate
        tag_data.put_slice(&Self::build_amf_string("audiosamplerate", 44100.0)); // audiosamplerate
        tag_data.put_slice(&Self::build_amf_bool("stereo", true)); // stereo
        tag_data.put_slice(&Self::build_amf_string("audiocodecid", 10.0)); // audiocodecid (10 for AAC)

        tag_data.put_u8(0x00); // object end marker
        tag_data.put_u8(0x00);
        tag_data.put_u8(0x09);

        tag_data.freeze()
    }

    fn build_amf_string(key: &str, value: f64) -> Vec<u8> {
        let mut amf = Vec::new();
        amf.push(0x02); // type: string
        amf.extend(&(key.len() as u16).to_be_bytes()); // length
        amf.extend(key.as_bytes()); // string

        amf.push(0x00); // type: number
        amf.extend(&value.to_be_bytes()); // value
        amf
    }

    fn build_amf_bool(key: &str, value: bool) -> Vec<u8> {
        let mut amf = Vec::new();
        amf.push(0x02); // type: string
        amf.extend(&(key.len() as u16).to_be_bytes()); // length
        amf.extend(key.as_bytes()); // string

        amf.push(0x01); // type: bool
        amf.push(if value { 0x01 } else { 0x00 }); // value
        amf
    }
}