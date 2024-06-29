use std::collections::HashMap;
use std::io::Cursor;

use byteorder::{BigEndian, WriteBytesExt};
use log::warn;
use rml_amf0::{Amf0Value, deserialize, serialize};

use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use constructor::{New, Set};

use crate::container::HandleMuxerDataFn;

const FLV_MTU: usize = 1200;

//+----------------+-----------------+
// | Header         | Data            |
// +----------------+-----------------+
// | Frame Type     | Codec ID        |
// | AVCPacketType  | CompositionTime |
// | NALU Length    | NALU Data       |
// | NALU Length    | NALU Data       |
// | ...            | ...             |
// +----------------+-----------------+
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

pub struct PreviousTagSize(u32);

impl PreviousTagSize {
    pub fn previous_tag_size_0() -> Bytes {
        Bytes::from(vec![0u8, 0, 0, 0])
    }
    pub fn previous_tag_size(self) -> Bytes {
        Bytes::from(self.0.to_be_bytes().to_vec())
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

///flv body start
pub struct TagHeader {
    tag_type: u8,
    data_size: [u8; 3],
    timestamp: [u8; 3],
    timestamp_ext: u8,
    stream_id: [u8; 3],
}

impl TagHeader {
    pub fn build(tag_type: TagType, ts: u32, data_size: u32) -> Self {
        let data_size_arr = data_size.to_be_bytes();
        let ts_arr = ts.to_be_bytes();
        Self {
            tag_type: tag_type.get_value(),
            data_size: [data_size_arr[1], data_size_arr[2], data_size_arr[3]], //大端
            timestamp: [ts_arr[1], ts_arr[2], ts_arr[3]],
            timestamp_ext: ts_arr[0],
            stream_id: [0x00, 0x00, 0x00],
        }
    }
    pub fn to_bytes(self) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put_u8(self.tag_type); //TagType: TagType：09（Tag的类型，包括音频（0x08）、视频（0x09）、script data（0x12）） 1byte
        bm.put_slice(&self.data_size); //Tag Data 大小 3 bytes
        bm.put_slice(&self.timestamp); //时间戳地位3位，大端 3bytes
        bm.put_u8(self.timestamp_ext); //时间戳的扩展部分，高位 1bytes
        bm.put_slice(&self.stream_id); //总是0 3 bytes
        bm.freeze()
    }
}

#[derive(New)]
pub struct ScriptData {
    metadata: ScriptMetaData,
}

impl ScriptData {
    pub fn to_bytes(self) -> GlobalResult<Bytes> {
        self.metadata.to_bytes()
    }
}

#[derive(Default, Debug, Set)]
pub struct ScriptMetaData {
    duration: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    videodatarate: Option<f64>,
    framerate: Option<f64>,
    videocodecid: Option<f64>,
    audiodatarate: Option<f64>,
    audiosamplerate: Option<f64>,
    audiosamplesize: Option<f64>,
    stereo: Option<bool>,
    audiocodecid: Option<f64>,
    filesize: Option<f64>,
}

impl ScriptMetaData {
    pub fn to_bytes(&self) -> GlobalResult<Bytes> {
        let mut properties = HashMap::new();
        if let Some(duration) = self.duration {
            properties.insert("duration".to_string(), Amf0Value::Number(duration));
        }
        if let Some(width) = self.width {
            properties.insert("width".to_string(), Amf0Value::Number(width));
        }
        if let Some(height) = self.height {
            properties.insert("height".to_string(), Amf0Value::Number(height));
        }
        if let Some(videodatarate) = self.videodatarate {
            properties.insert("videodatarate".to_string(), Amf0Value::Number(videodatarate));
        }
        if let Some(framerate) = self.framerate {
            properties.insert("framerate".to_string(), Amf0Value::Number(framerate));
        }
        if let Some(videocodecid) = self.videocodecid {
            properties.insert("videocodecid".to_string(), Amf0Value::Number(videocodecid));
        }
        if let Some(audiodatarate) = self.audiodatarate {
            properties.insert("audiodatarate".to_string(), Amf0Value::Number(audiodatarate));
        }
        if let Some(audiosamplerate) = self.audiosamplerate {
            properties.insert("audiosamplerate".to_string(), Amf0Value::Number(audiosamplerate));
        }
        if let Some(stereo) = self.stereo {
            properties.insert("stereo".to_string(), Amf0Value::Boolean(stereo));
        }
        if let Some(audiocodecid) = self.audiocodecid {
            properties.insert("audiocodecid".to_string(), Amf0Value::Number(audiocodecid));
        }
        if let Some(filesize) = self.filesize {
            properties.insert("filesize".to_string(), Amf0Value::Number(filesize));
        }
        let amf1 = Amf0Value::Utf8String("onMetaData".to_string());
        let amf2 = Amf0Value::Object(properties);
        let bytes = serialize(&vec![amf1, amf2]).hand_log(|msg| warn!("{msg}"))?;
        Ok(Bytes::from(bytes))
    }
}

#[derive(New)]
pub struct ScripTag {
    tag_header: TagHeader,
    tag_data: ScriptData,
}

impl ScripTag {
    pub fn to_bytes(self) -> GlobalResult<Bytes> {
        let mut bm = BytesMut::new();
        bm.put(self.tag_header.to_bytes());
        bm.put(self.tag_data.to_bytes()?);
        Ok(bm.freeze())
    }
}

pub struct AudioTagDataFirst {}

pub struct AudioTagDataLast {}


pub struct VideoTagDataFirst {
    frame_type_codec_id: u8,
    avc_packet_type: u8,
    composition_time_offset: u32,

}

pub struct AVCDecoderConfiguration{

}

pub struct VideoTagDataLast {
    tag_header: TagHeader,
    tag_data: VideoTagData,
}

#[derive(New)]
pub struct VideoTagData {
    frame_type_codec_id: u8,
    avc_packet_type: u8,
    composition_time_offset: u32,
    data: Bytes,
}

impl VideoTagData {
    pub fn to_bytes(self) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put_u8(self.frame_type_codec_id);
        bm.put_u8(self.avc_packet_type);
        bm.put_slice(&self.composition_time_offset.to_be_bytes()[1..=3]);
        bm.put(self.data);
        bm.freeze()
    }
}

pub struct VideoTagDataBuffer {
    sei: Option<Bytes>, //6
    sps: Option<Bytes>, //7
    pps: Option<Bytes>, //8
    aud: Option<Bytes>, //9
    // idr: Option<Bytes>, //5
    // other_frame: Option<Bytes>,
}

impl VideoTagDataBuffer {
    pub fn init() -> Self {
        Self {
            sps: None,
            pps: None,
            sei: None,
            aud: None,
        }
    }

    pub fn packaging(&mut self, nal: Bytes) -> Option<VideoTagData> {
        match nal[4] & 0x1F {
            5 => {
                let mut bm = BytesMut::new();
                if let Some(sps) = &self.sps {
                    bm.put(sps.clone());
                }
                if let Some(pps) = &self.pps {
                    bm.put(pps.clone());
                }
                if let Some(aud) = &mut self.aud {
                    bm.put(std::mem::take(aud));
                }
                if let Some(sei) = &mut self.sei {
                    bm.put(std::mem::take(sei));
                }
                bm.put(nal);
                let data = bm.freeze();
                let video_tag_data = VideoTagData::new(17, 1, 0, data);
                Some(video_tag_data)
            }
            6 => {
                self.sei = Some(nal);
                None
            }
            7 => {
                self.sps = Some(nal);
                None
            }
            8 => {
                self.pps = Some(nal);
                None
            }
            9 => {
                self.aud = Some(nal);
                None
            }
            _ => {
                let mut bm = BytesMut::new();
                if let Some(aud) = &mut self.aud {
                    bm.put(std::mem::take(aud));
                }
                if let Some(sei) = &mut self.sei {
                    bm.put(std::mem::take(sei));
                }
                bm.put(nal);
                let data = bm.freeze();
                let video_tag_data = VideoTagData::new(27, 1, 0, data);
                Some(video_tag_data)
            }
        }
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
    //1.是否全局判断设置frame_type_codec_id
    //2.sps pps
    pub fn process(f: HandleMuxerDataFn, tag_type: TagType, ts: u32, data: Bytes, frame_type_codec_id: u8) -> GlobalResult<()> {
        // let mut frame_type_codec_id = 0x27u8;
        // if matches!(data[4] &0x1F,7|8|5) { //判断是否为关键帧
        //     frame_type_codec_id = 17;
        // }
        let sub_tag_bytes = FlvTag::build(tag_type, ts, data).to_bytes(frame_type_codec_id);
        let len_vec = (sub_tag_bytes.len() as u32).to_be_bytes().to_vec();
        let previos_tag_size = Bytes::from(len_vec);
        f(sub_tag_bytes)?;
        f(previos_tag_size)?;
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

    fn to_bytes(self, frame_type_codec_id: u8) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put_u8(self.tag_type); //TagType: TagType：09（Tag的类型，包括音频（0x08）、视频（0x09）、script data（0x12）） 1byte
        bm.put_slice(&self.data_size); //Tag Data 大小 3 bytes
        bm.put_slice(&self.timestamp); //时间戳地位3位，大端 3bytes
        bm.put_u8(self.timestamp_ext); //时间戳的扩展部分，高位 1bytes
        bm.put_slice(&[0x00, 0x00, 0x00]); //总是0 3 bytes
        //FrameType:
        //     1: keyframe (for AVC, a seekableframe)
        //     2: inter frame(for AVC, a non -seekable frame)
        //     3 : disposable inter frame(H.263only)
        //     4 : generated keyframe(reserved forserver use only)
        //     5 : video info / command frame
        //CodecID:
        //     1: JPEG (currently unused)
        //     2: Sorenson H.263
        //     3 : Screen video
        //     4 : On2 VP6
        //     5 : On2 VP6 with alpha channel
        //     6 : Screen video version 2
        //     7 : AVC  H.264 的正式名称，全称是 Advanced Video Coding
        bm.put_u8(frame_type_codec_id); //FrameType 4bit + CodecID 4bit 共1byte；
        bm.put_u8(1); //AVCPaketType：0: AVC sequence header; 1: AVC NALU; 2: AVC end of sequence
        bm.put_slice(&[0u8, 0, 0]); //CompositionTime Offset
        bm.put(&self.data[..]);
        bm.freeze()
    }
}


#[derive(New)]
pub struct AVCDecoderConfiguration1 {
    sps: Bytes,
    pps: Bytes,
    ts: u32,
}

impl AVCDecoderConfiguration1 {
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

        FlvTag::build(TagType::Video, self.ts, video_tag_data.freeze()).to_bytes(0x17)
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
        packet.put_slice(&[0u8, 0, 0]); // timestamp
        packet.put_u32(0); // stream ID
        packet.put(tag_data);
        // PreviousTagSize
        packet.put_u32(11 + data_size);
        packet.freeze()
    }

    pub fn build_script_tag_data(width: u32, height: u32, framerate: f64) -> GlobalResult<Bytes> {
        let amf1 = Amf0Value::Utf8String("onMetaData".to_string());
        let mut properties = HashMap::new();
        properties.insert("width".to_string(), Amf0Value::Number(width as f64));
        properties.insert("height".to_string(), Amf0Value::Number(width as f64));
        properties.insert("videocodecid".to_string(), Amf0Value::Number(7.0)); //videocodecid (7 for AVC)
        properties.insert("framerate".to_string(), Amf0Value::Number(width as f64));
        let amf2 = Amf0Value::Object(properties);
        let bytes = serialize(&vec![amf1, amf2]).hand_log(|msg| warn!("{msg}"))?;
        Ok(Bytes::from(bytes))
        // let mut tag_data = BytesMut::new();
        // ECMA array with metadata
        // tag_data.put_u8(0x02); // type: string
        // tag_data.put_u16(0x0A); // length: 10
        // tag_data.put_slice(b"onMetaData"); // string: "onMetaData"
        // tag_data.put_u8(0x08); // type: ECMA array
        // tag_data.put_u32(0x00_00_00_05); // number of elements: 10: 0x00_00_00_0A
        // tag_data.put_slice(&Self::build_amf_string("duration", 0.0)); // duration
        // tag_data.put_slice(&Self::build_amf_string("width", width as f64)); // width
        // tag_data.put_slice(&Self::build_amf_string("height", height as f64)); // height
        // tag_data.put_slice(&Self::build_amf_string("videodatarate", 5000.0)); // videodatarate
        // tag_data.put_slice(&Self::build_amf_string("framerate", framerate)); // framerate
        // tag_data.put_slice(&Self::build_amf_string("videocodecid", 7.0)); // videocodecid (7 for AVC)
        // tag_data.put_slice(&Self::build_amf_string("audiodatarate", 128.0)); // audiodatarate
        // tag_data.put_slice(&Self::build_amf_string("audiosamplerate", 44100.0)); // audiosamplerate
        // tag_data.put_slice(&Self::build_amf_bool("stereo", true)); // stereo
        // tag_data.put_slice(&Self::build_amf_string("audiocodecid", 10.0)); // audiocodecid (10 for AAC)
        // tag_data.put_u8(0x00); // object end marker
        // tag_data.put_u8(0x00);
        // tag_data.put_u8(0x09);
        // tag_data.freeze()
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

#[test]
fn test_amf0_script() {
    let bytes = [0x02, 0x00, 0x0A, 0x6F, 0x6E, 0x4D, 0x65, 0x74, 0x61, 0x44, 0x61, 0x74, 0x61, 0x08, 0x00, 0x00, 0x00, 0x10, 0x00, 0x08, 0x64, 0x75, 0x72, 0x61, 0x74, 0x69, 0x6F, 0x6E, 0x00, 0x40, 0x40, 0x85, 0x60, 0x41, 0x89, 0x37, 0x4C, 0x00, 0x05, 0x77, 0x69, 0x64, 0x74, 0x68, 0x00, 0x40, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x68, 0x65, 0x69, 0x67, 0x68, 0x74, 0x00, 0x40, 0x70, 0xE0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D, 0x76, 0x69, 0x64, 0x65, 0x6F, 0x64, 0x61, 0x74, 0x61, 0x72, 0x61, 0x74, 0x65, 0x00, 0x40, 0x89, 0x2B, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x66, 0x72, 0x61, 0x6D, 0x65, 0x72, 0x61, 0x74, 0x65, 0x00, 0x40, 0x39, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x76, 0x69, 0x64, 0x65, 0x6F, 0x63, 0x6F, 0x64, 0x65, 0x63, 0x69, 0x64, 0x00, 0x40, 0x1C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D, 0x61, 0x75, 0x64, 0x69, 0x6F, 0x64, 0x61, 0x74, 0x61, 0x72, 0x61, 0x74, 0x65, 0x00, 0x40, 0x5F, 0x20, 0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0F, 0x61, 0x75, 0x64, 0x69, 0x6F, 0x73, 0x61, 0x6D, 0x70, 0x6C, 0x65, 0x72, 0x61, 0x74, 0x65, 0x00, 0x40, 0xE5, 0x88, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0F, 0x61, 0x75, 0x64, 0x69, 0x6F, 0x73, 0x61, 0x6D, 0x70, 0x6C, 0x65, 0x73, 0x69, 0x7A, 0x65, 0x00, 0x40, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x73, 0x74, 0x65, 0x72, 0x65, 0x6F, 0x01, 0x01, 0x00, 0x0C, 0x61, 0x75, 0x64, 0x69, 0x6F, 0x63, 0x6F, 0x64, 0x65, 0x63, 0x69, 0x64, 0x00, 0x40, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0B, 0x6D, 0x61, 0x6A, 0x6F, 0x72, 0x5F, 0x62, 0x72, 0x61, 0x6E, 0x64, 0x02, 0x00, 0x04, 0x4D, 0x34, 0x56, 0x50, 0x00, 0x0D, 0x6D, 0x69, 0x6E, 0x6F, 0x72, 0x5F, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6F, 0x6E, 0x02, 0x00, 0x01, 0x31, 0x00, 0x11, 0x63, 0x6F, 0x6D, 0x70, 0x61, 0x74, 0x69, 0x62, 0x6C, 0x65, 0x5F, 0x62, 0x72, 0x61, 0x6E, 0x64, 0x73, 0x02, 0x00, 0x10, 0x4D, 0x34, 0x56, 0x50, 0x4D, 0x34, 0x41, 0x20, 0x6D, 0x70, 0x34, 0x32, 0x69, 0x73, 0x6F, 0x6D, 0x00, 0x07, 0x65, 0x6E, 0x63, 0x6F, 0x64, 0x65, 0x72, 0x02, 0x00, 0x0D, 0x4C, 0x61, 0x76, 0x66, 0x35, 0x36, 0x2E, 0x34, 0x30, 0x2E, 0x31, 0x30, 0x30, 0x00, 0x08, 0x66, 0x69, 0x6C, 0x65, 0x73, 0x69, 0x7A, 0x65, 0x00, 0x41, 0x4D, 0xE0, 0x46, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09];
    let mut serialized_cursor = Cursor::new(bytes);
    let res = deserialize(&mut serialized_cursor).unwrap();
    for amf in res {
        println!("{:?}", &amf);
    }
}

#[test]
fn test_bytes_mut() {
    let mut bm = BytesMut::new();
    let mut bytes = Bytes::copy_from_slice("abc".as_ref());
    bm.put(&mut bytes);
    println!("bm len = {}", bm.len());
    println!("b len = {}", bytes.len());
}