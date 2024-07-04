use log::warn;
use amf::{Pair};
use amf::amf0::Value;
use common::bytes::{BufMut, Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use constructor::{New, Set};

pub struct MediaFlvContainer {
    pub flv_video_h264: VideoTagDataBuffer,
}

impl MediaFlvContainer {
    pub fn register_all() -> Self {
        Self { flv_video_h264: VideoTagDataBuffer::init() }
    }
}

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

    pub fn build(video: bool, audio: bool) -> Self {
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

    pub fn to_bytes(self) -> Bytes {
        let mut bm = BytesMut::new();
        bm.put(&self.signature[..]);
        bm.put_u8(self.version);
        bm.put_u8(self.flags);
        bm.put_u32(self.header_length);
        bm.put_u32(0);
        bm.freeze()
    }
}

#[derive(New)]
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

pub struct ScriptData {
    // amf1: Bytes,
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
        let mut arr = Vec::<Pair<String, Value>>::new();
        if let Some(duration) = self.duration {
            let pair = Pair { key: "duration".to_string(), value: Value::Number(duration) };
            arr.push(pair);
        }
        if let Some(width) = self.width {
            let pair = Pair { key: "width".to_string(), value: Value::Number(width) };
            arr.push(pair);
        }
        if let Some(height) = self.height {
            let pair = Pair { key: "height".to_string(), value: Value::Number(height) };
            arr.push(pair);
        }
        if let Some(videodatarate) = self.videodatarate {
            let pair = Pair { key: "videodatarate".to_string(), value: Value::Number(videodatarate) };
            arr.push(pair);
        }
        if let Some(framerate) = self.framerate {
            let pair = Pair { key: "framerate".to_string(), value: Value::Number(framerate) };
            arr.push(pair);
        }
        if let Some(videocodecid) = self.videocodecid {
            let pair = Pair { key: "videocodecid".to_string(), value: Value::Number(videocodecid) };
            arr.push(pair);
        }
        if let Some(audiodatarate) = self.audiodatarate {
            let pair = Pair { key: "audiodatarate".to_string(), value: Value::Number(audiodatarate) };
            arr.push(pair);
        }
        if let Some(audiosamplerate) = self.audiosamplerate {
            let pair = Pair { key: "audiosamplerate".to_string(), value: Value::Number(audiosamplerate) };
            arr.push(pair);
        }
        if let Some(stereo) = self.stereo {
            let pair = Pair { key: "stereo".to_string(), value: Value::Boolean(stereo) };
            arr.push(pair);
        }
        if let Some(audiocodecid) = self.audiocodecid {
            let pair = Pair { key: "audiocodecid".to_string(), value: Value::Number(audiocodecid) };
            arr.push(pair);
        }
        if let Some(filesize) = self.filesize {
            let pair = Pair { key: "filesize".to_string(), value: Value::Number(filesize) };
            arr.push(pair);
        }
        let mut buf = Vec::new();
        let amf1 = Value::from(Value::String("onMetaData".to_string()));
        amf1.write_to(&mut buf).hand_log(|msg| warn!("{msg}"))?;
        let amf2 = Value::EcmaArray { entries: arr };
        amf2.write_to(&mut buf).hand_log(|msg| warn!("{msg}"))?;
        Ok(Bytes::from(buf))
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
    composition_time_offset: [u8; 3],
    avc_decoder_configuration_record: AvcDecoderConfigurationRecord,
}

impl VideoTagDataFirst {
    pub fn build(avc_decoder_configuration_record: AvcDecoderConfigurationRecord) -> Self {
        Self {
            frame_type_codec_id: 0x17,
            avc_packet_type: 0,
            composition_time_offset: [0, 0, 0],
            avc_decoder_configuration_record,
        }
    }

    pub fn to_bytes(self) -> Bytes {
        let mut bytes = BytesMut::new();
        bytes.put_u8(self.frame_type_codec_id);
        bytes.put_u8(self.avc_packet_type);
        bytes.put_slice(&self.composition_time_offset);
        bytes.put(self.avc_decoder_configuration_record.to_bytes());
        bytes.freeze()
    }
}

pub struct AvcDecoderConfigurationRecord {
    configuration_version: u8,
    avc_profile_indication: u8,
    profile_compatibility: u8,
    avc_level_indication: u8,
    reserved6bit_length_size_minus_one2bit: u8,
    reserved3bit_num_of_sequence_parameter_sets5bit: u8,
    sequence_parameter_set_length: u16,
    sequence_parameter_set_nal_units: Bytes,
    num_of_picture_parameter_sets: u8,
    picture_parameter_set_length: u16,
    picture_parameter_set_nal_units: Bytes,
}

impl AvcDecoderConfigurationRecord {
    pub fn build(sps: Bytes, pps: Bytes) -> Self {
        Self {
            configuration_version: 1,
            avc_profile_indication: sps[1],
            profile_compatibility: sps[2],
            avc_level_indication: sps[3],
            reserved6bit_length_size_minus_one2bit: 0xff,
            reserved3bit_num_of_sequence_parameter_sets5bit: 0xe1,
            sequence_parameter_set_length: sps.len() as u16,
            sequence_parameter_set_nal_units: sps,
            num_of_picture_parameter_sets: 1,
            picture_parameter_set_length: pps.len() as u16,
            picture_parameter_set_nal_units: pps,
        }
    }
    //todo 宏实现
    pub fn to_bytes(self) -> Bytes {
        let mut bytes = BytesMut::new();
        bytes.put_u8(self.configuration_version);
        bytes.put_u8(self.avc_profile_indication);
        bytes.put_u8(self.profile_compatibility);
        bytes.put_u8(self.avc_level_indication);
        bytes.put_u8(self.reserved6bit_length_size_minus_one2bit);
        bytes.put_u8(self.reserved3bit_num_of_sequence_parameter_sets5bit);
        bytes.put_u16(self.sequence_parameter_set_length);
        bytes.put(self.sequence_parameter_set_nal_units);
        bytes.put_u8(self.num_of_picture_parameter_sets);
        bytes.put_u16(self.picture_parameter_set_length);
        bytes.put(self.picture_parameter_set_nal_units);
        bytes.freeze()
    }
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
        let mut sps_vec = base64::decode("Z00AKpWoHgCJ+VA=").unwrap();
        let sps_len = sps_vec.len() as u32;
        let s = sps_len.to_be_bytes();
        sps_vec.insert(0, s[0]);
        sps_vec.insert(1, s[1]);
        sps_vec.insert(2, s[2]);
        sps_vec.insert(3, s[3]);
        let sps = Bytes::from(sps_vec);

        let mut pps_vec = base64::decode("aO48gA==").unwrap();
        let pps_len = pps_vec.len() as u32;
        let p = pps_len.to_be_bytes();
        pps_vec.insert(0, p[0]);
        pps_vec.insert(1, p[1]);
        pps_vec.insert(2, p[2]);
        pps_vec.insert(3, p[3]);
        let pps = Bytes::from(pps_vec);

        Self {
            sps: Some(sps),
            pps: Some(pps),
            sei: None,
            aud: None,
        }
    }

    pub fn packaging(&mut self, nal: Bytes) -> Option<VideoTagData> {
        match nal[4] & 0x1F {
            5 => {
                let mut bm = BytesMut::new();
                if let (Some(sps), Some(pps)) = (&self.sps, &self.pps) {
                    bm.put(sps.clone());
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
                let video_tag_data = VideoTagData::new(0x17, 1, 0, data);
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
                let video_tag_data = VideoTagData::new(0x27, 1, 0, data);
                Some(video_tag_data)
            }
        }
    }
}


// pub struct FlvTag {
//     tag_type: u8,
//     data_size: [u8; 3],
//     timestamp: [u8; 3],
//     timestamp_ext: u8,
//     stream_id: u32,
//     data: Bytes,
// }
//
// impl FlvTag {
//     fn build(tag_type: TagType, ts: u32, data: Bytes) -> Self {
//         let size_arr = (data.len() as u32).to_be_bytes();
//         let data_size = [size_arr[1], size_arr[2], size_arr[3]];
//         let ts_arr = ts.to_be_bytes();
//         Self {
//             tag_type: tag_type.get_value(),
//             data_size,
//             timestamp: [ts_arr[1], ts_arr[2], ts_arr[3]],
//             timestamp_ext: ts_arr[0],
//             stream_id: 0,
//             data,
//         }
//     }
//
//     fn to_bytes(self, frame_type_codec_id: u8) -> Bytes {
//         let mut bm = BytesMut::new();
//         bm.put_u8(self.tag_type); //TagType: TagType：09（Tag的类型，包括音频（0x08）、视频（0x09）、script data（0x12）） 1byte
//         bm.put_slice(&self.data_size); //Tag Data 大小 3 bytes
//         bm.put_slice(&self.timestamp); //时间戳地位3位，大端 3bytes
//         bm.put_u8(self.timestamp_ext); //时间戳的扩展部分，高位 1bytes
//         bm.put_slice(&[0x00, 0x00, 0x00]); //总是0 3 bytes
//         //FrameType:
//         //     1: keyframe (for AVC, a seekableframe)
//         //     2: inter frame(for AVC, a non -seekable frame)
//         //     3 : disposable inter frame(H.263only)
//         //     4 : generated keyframe(reserved forserver use only)
//         //     5 : video info / command frame
//         //CodecID:
//         //     1: JPEG (currently unused)
//         //     2: Sorenson H.263
//         //     3 : Screen video
//         //     4 : On2 VP6
//         //     5 : On2 VP6 with alpha channel
//         //     6 : Screen video version 2
//         //     7 : AVC  H.264 的正式名称，全称是 Advanced Video Coding
//         bm.put_u8(frame_type_codec_id); //FrameType 4bit + CodecID 4bit 共1byte；
//         bm.put_u8(1); //AVCPaketType：0: AVC sequence header; 1: AVC NALU; 2: AVC end of sequence
//         bm.put_slice(&[0u8, 0, 0]); //CompositionTime Offset
//         bm.put(&self.data[..]);
//         bm.freeze()
//     }
// }

#[cfg(test)]
mod test {
    use byteorder::{BigEndian, ByteOrder};
    use crate::container::flv::ScriptMetaData;

    #[test]
    fn test_metadata() {
        let meta_data = ScriptMetaData {
            duration: Some(123f64),
            width: Some(1920f64),
            height: Some(720f64),
            videodatarate: None,
            framerate: Some(25f64),
            videocodecid: Some(7f64),
            audiodatarate: None,
            audiosamplerate: None,
            audiosamplesize: None,
            stereo: None,
            audiocodecid: None,
            filesize: None,
        };
        match meta_data.to_bytes() {
            Ok(meta) => {
                println!("{:02x}", meta);
            }
            Err(err) => {
                println!("{:?}", err);
            }
        }
    }

    #[test]
    fn test_flv_data() {
        let sps_vec = base64::decode("Z00AKpWoHgCJ+VA=").unwrap();
        let pps_vec = base64::decode("aO48gA==").unwrap();
        println!("{:02x?}", sps_vec);
        println!("{:02x?}", pps_vec);
        let input = include_bytes!("/home/ubuntu20/code/rs/mv/github/epimore/12.flv");
        println!("input size = {}", input.len());
        let input = &input[187..52401];
        let mut curr_offset = 0;
        let size_len = 4;
        while curr_offset < input.len() {
            let data_size = u32::from_be_bytes([input[curr_offset], input[curr_offset + 1], input[curr_offset + 2], input[curr_offset + 3]]) as usize;
            println!("nal len {data_size}, type = {:02x}", input[curr_offset + size_len]);
            curr_offset += size_len + data_size;
        }
    }

    #[test]
    fn byte_to_number() {
        let fbytes = [0x40, 0x9E, 00, 00, 00, 00, 00, 00];
        println!("f64 = {}", BigEndian::read_f64(&fbytes));

        let nbytes = [0x00, 0x02, 0x59,0xD3];
        println!("num = {}",BigEndian::read_u32(&nbytes));
    }
}
//ypedef struct ScriptTagData
// {
//   unsigned char MetaDataType;//0x02
//   unsigned char StringLenth[2];//一般位10，即0x000A；
//   unsigned char MetaString[10];//值为onMetaDat
//   unsigned char InfoDataType;//0x08表示数组，也就是第二个AMF包
//   unsigned char EnumNum[4];//4bytes有多少个元素//18bytes
//   //1
//   unsigned char DurationLenth[2];//2bytes,duration的长度
//   unsigned char DurationName[8];
//   unsigned char DurationType;
//   unsigned char DurationData[8];
//   //2
//   unsigned char WidthLenth[2];//
//   unsigned char WidthName[5];
//   unsigned char WidthType;
//   unsigned char WidthData[8];
//   //3
//   unsigned char HeightLenth[2];
//   unsigned char HeightName[6];
//   unsigned char HeightType;
//   unsigned char HeightData[8];
//   //4
//   unsigned char FrameRateLenth[2];
//   unsigned char FrameRateName[9];
//   unsigned char FrameRateType;
//   unsigned char FrameRateData[8];
//   //5
//   unsigned char FileSizeLenth[2];
//   unsigned char FileSizeName[8];
//   unsigned char FileSizeType;
//   unsigned char FileSizeData[8];
//
//   unsigned char End[3];//0x000009
// }ScriptTagData;