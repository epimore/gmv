// Phase 1 keeps packetization isolated from the production sender until Phase 4 negotiation lands.
#![allow(dead_code)]

use std::error::Error;
use std::fmt::{Display, Formatter};

const RTP_HEADER_LEN: usize = 12;
const PS_CLOCK_RATE: u32 = 90_000;
const PCMA_SAMPLE_RATE: u32 = 8_000;
const AUDIO_STREAM_ID: u8 = 0xc0;
const PCMA_STREAM_TYPE: u8 = 0x90;
const PROGRAM_MUX_RATE: u32 = 512;
const AUDIO_BUFFER_BOUND: u16 = 32;
const PROGRAM_STREAM_MAP_INTERVAL_FRAMES: u64 = 30;

pub(crate) const DEFAULT_MAX_RTP_PAYLOAD_LEN: usize = 1_400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PsRtpPacketizerConfig {
    pub payload_type: u8,
    pub ssrc: u32,
    pub sequence: u16,
    pub timestamp: u32,
    pub frame_duration_ms: u16,
    pub max_rtp_payload_len: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PsPacketizerError {
    InvalidPayloadType(u8),
    InvalidFrameDuration(u16),
    InvalidMaxPayloadLen(usize),
    InvalidFrameLength { expected: usize, actual: usize },
    Finished,
}

impl Display for PsPacketizerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayloadType(value) => {
                write!(formatter, "RTP payload type must be in 0..=127: {value}")
            }
            Self::InvalidFrameDuration(value) => {
                write!(formatter, "invalid PCMA frame duration: {value}ms")
            }
            Self::InvalidMaxPayloadLen(value) => {
                write!(formatter, "invalid maximum RTP payload length: {value}")
            }
            Self::InvalidFrameLength { expected, actual } => write!(
                formatter,
                "invalid PCMA frame length: expected {expected}, got {actual}"
            ),
            Self::Finished => formatter.write_str("PS/RTP packetizer is already finished"),
        }
    }
}

impl Error for PsPacketizerError {}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PacketizedRtp {
    pub sequence: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub bytes: Vec<u8>,
}

pub(crate) struct PsRtpPacketizer {
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    timeline_90k: u64,
    timestamp_step: u32,
    frame_len: usize,
    max_rtp_payload_len: usize,
    frame_index: u64,
    ps_buffer: Vec<u8>,
    finished: bool,
}

impl PsRtpPacketizer {
    pub fn new(config: PsRtpPacketizerConfig) -> Result<Self, PsPacketizerError> {
        if config.payload_type > 127 {
            return Err(PsPacketizerError::InvalidPayloadType(config.payload_type));
        }
        if !(10..=60).contains(&config.frame_duration_ms)
            || PCMA_SAMPLE_RATE * u32::from(config.frame_duration_ms) % 1_000 != 0
        {
            return Err(PsPacketizerError::InvalidFrameDuration(
                config.frame_duration_ms,
            ));
        }
        if config.max_rtp_payload_len == 0
            || config.max_rtp_payload_len > usize::from(u16::MAX) - RTP_HEADER_LEN
        {
            return Err(PsPacketizerError::InvalidMaxPayloadLen(
                config.max_rtp_payload_len,
            ));
        }

        let timestamp_step = PS_CLOCK_RATE * u32::from(config.frame_duration_ms) / 1_000;
        let frame_len = (PCMA_SAMPLE_RATE * u32::from(config.frame_duration_ms) / 1_000) as usize;
        Ok(Self {
            payload_type: config.payload_type,
            ssrc: config.ssrc,
            sequence: config.sequence,
            timestamp: config.timestamp,
            timeline_90k: u64::from(config.timestamp),
            timestamp_step,
            frame_len,
            max_rtp_payload_len: config.max_rtp_payload_len,
            frame_index: 0,
            ps_buffer: Vec::with_capacity(512),
            finished: false,
        })
    }

    pub fn packetize(&mut self, pcma: &[u8]) -> Result<Vec<PacketizedRtp>, PsPacketizerError> {
        if self.finished {
            return Err(PsPacketizerError::Finished);
        }
        if pcma.len() != self.frame_len {
            return Err(PsPacketizerError::InvalidFrameLength {
                expected: self.frame_len,
                actual: pcma.len(),
            });
        }

        self.ps_buffer.clear();
        let pts = self.timeline_90k & ((1_u64 << 33) - 1);
        write_pack_header(&mut self.ps_buffer, pts);
        if self.frame_index % PROGRAM_STREAM_MAP_INTERVAL_FRAMES == 0 {
            write_system_header(&mut self.ps_buffer);
            write_program_stream_map(&mut self.ps_buffer);
        }
        write_audio_pes(&mut self.ps_buffer, pts, pcma);

        let fragment_count = self.ps_buffer.len().div_ceil(self.max_rtp_payload_len);
        let mut packets = Vec::with_capacity(fragment_count);
        for (index, payload) in self.ps_buffer.chunks(self.max_rtp_payload_len).enumerate() {
            let marker = index + 1 == fragment_count;
            let sequence = self.sequence.wrapping_add(index as u16);
            packets.push(PacketizedRtp {
                sequence,
                timestamp: self.timestamp,
                marker,
                bytes: build_rtp_packet(
                    self.payload_type,
                    self.ssrc,
                    sequence,
                    self.timestamp,
                    marker,
                    payload,
                ),
            });
        }

        self.sequence = self.sequence.wrapping_add(fragment_count as u16);
        self.timestamp = self.timestamp.wrapping_add(self.timestamp_step);
        self.timeline_90k =
            (self.timeline_90k + u64::from(self.timestamp_step)) & ((1_u64 << 33) - 1);
        self.frame_index += 1;
        Ok(packets)
    }

    pub fn finish(&mut self) -> Vec<PacketizedRtp> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if self.frame_index == 0 {
            return Vec::new();
        }

        let payload = [0x00, 0x00, 0x01, 0xb9];
        let packet = PacketizedRtp {
            sequence: self.sequence,
            timestamp: self.timestamp,
            marker: true,
            bytes: build_rtp_packet(
                self.payload_type,
                self.ssrc,
                self.sequence,
                self.timestamp,
                true,
                &payload,
            ),
        };
        self.sequence = self.sequence.wrapping_add(1);
        vec![packet]
    }

    #[cfg(test)]
    fn ps_buffer_capacity(&self) -> usize {
        self.ps_buffer.capacity()
    }
}

fn write_pack_header(output: &mut Vec<u8>, scr_90k: u64) {
    let scr = scr_90k & ((1_u64 << 33) - 1);
    output.extend_from_slice(&[0x00, 0x00, 0x01, 0xba]);
    output.push(0x44 | (((scr >> 30) as u8 & 0x07) << 3) | ((scr >> 28) as u8 & 0x03));
    output.push((scr >> 20) as u8);
    output.push((((scr >> 15) as u8 & 0x1f) << 3) | 0x04 | ((scr >> 13) as u8 & 0x03));
    output.push((scr >> 5) as u8);
    output.push(((scr as u8 & 0x1f) << 3) | 0x04);
    output.push(0x01);
    output.push((PROGRAM_MUX_RATE >> 14) as u8);
    output.push((PROGRAM_MUX_RATE >> 6) as u8);
    output.push(((PROGRAM_MUX_RATE as u8 & 0x3f) << 2) | 0x03);
    output.push(0xf8);
}

fn write_system_header(output: &mut Vec<u8>) {
    output.extend_from_slice(&[0x00, 0x00, 0x01, 0xbb, 0x00, 0x09]);
    output.push(0x80 | ((PROGRAM_MUX_RATE >> 15) as u8 & 0x7f));
    output.push((PROGRAM_MUX_RATE >> 7) as u8);
    output.push(((PROGRAM_MUX_RATE as u8 & 0x7f) << 1) | 0x01);
    output.push(0x04);
    output.push(0x20);
    output.push(0x7f);
    output.push(AUDIO_STREAM_ID);
    output.push(0xc0 | ((AUDIO_BUFFER_BOUND >> 8) as u8 & 0x1f));
    output.push(AUDIO_BUFFER_BOUND as u8);
}

fn write_program_stream_map(output: &mut Vec<u8>) {
    let start = output.len();
    output.extend_from_slice(&[
        0x00,
        0x00,
        0x01,
        0xbc,
        0x00,
        0x0e,
        0xc1,
        0xff,
        0x00,
        0x00,
        0x00,
        0x04,
        PCMA_STREAM_TYPE,
        AUDIO_STREAM_ID,
        0x00,
        0x00,
    ]);
    let crc = mpeg2_crc32(&output[start..]);
    output.extend_from_slice(&crc.to_be_bytes());
}

fn write_audio_pes(output: &mut Vec<u8>, pts_90k: u64, pcma: &[u8]) {
    let packet_len = 8 + pcma.len();
    debug_assert!(packet_len <= usize::from(u16::MAX));
    output.extend_from_slice(&[0x00, 0x00, 0x01, AUDIO_STREAM_ID]);
    output.extend_from_slice(&(packet_len as u16).to_be_bytes());
    output.extend_from_slice(&[0x84, 0x80, 0x05]);
    write_pts(output, pts_90k);
    output.extend_from_slice(pcma);
}

fn write_pts(output: &mut Vec<u8>, pts_90k: u64) {
    let pts = pts_90k & ((1_u64 << 33) - 1);
    output.push(0x21 | (((pts >> 30) as u8 & 0x07) << 1));
    output.push((pts >> 22) as u8);
    output.push((((pts >> 15) as u8 & 0x7f) << 1) | 0x01);
    output.push((pts >> 7) as u8);
    output.push(((pts as u8 & 0x7f) << 1) | 0x01);
}

fn build_rtp_packet(
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    marker: bool,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(RTP_HEADER_LEN + payload.len());
    packet.push(0x80);
    packet.push(payload_type | if marker { 0x80 } else { 0 });
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn mpeg2_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::time::{Duration, Instant, SystemTime};

    use rsmpeg::avformat::AVFormatContextInput;
    use rsmpeg::ffi::{AVCodecID_AV_CODEC_ID_PCM_ALAW, AVMediaType_AVMEDIA_TYPE_AUDIO};

    use crate::io::broadcast::{
        MAX_BROADCAST_LEGS_PER_NODE, MAX_BROADCAST_LEGS_PER_PARENT, MAX_BROADCAST_PARENTS_PER_NODE,
    };

    use super::{
        DEFAULT_MAX_RTP_PAYLOAD_LEN, PacketizedRtp, PsPacketizerError, PsRtpPacketizer,
        PsRtpPacketizerConfig, mpeg2_crc32,
    };

    fn config(max_rtp_payload_len: usize) -> PsRtpPacketizerConfig {
        PsRtpPacketizerConfig {
            payload_type: 96,
            ssrc: 0x1122_3344,
            sequence: 0x1234,
            timestamp: 0,
            frame_duration_ms: 20,
            max_rtp_payload_len,
        }
    }

    fn payload(packet: &PacketizedRtp) -> &[u8] {
        &packet.bytes[12..]
    }

    #[test]
    fn first_access_unit_matches_ps_golden_structure() {
        let mut packetizer = PsRtpPacketizer::new(config(DEFAULT_MAX_RTP_PAYLOAD_LEN)).unwrap();
        let packets = packetizer.packetize(&[0xd5; 160]).unwrap();
        assert_eq!(packets.len(), 1);
        let ps = payload(&packets[0]);
        assert_eq!(ps.len(), 223);
        assert_eq!(
            &ps[..14],
            &[
                0x00, 0x00, 0x01, 0xba, 0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x08, 0x03, 0xf8
            ]
        );
        assert_eq!(
            &ps[14..29],
            &[
                0x00, 0x00, 0x01, 0xbb, 0x00, 0x09, 0x80, 0x04, 0x01, 0x04, 0x20, 0x7f, 0xc0, 0xc0,
                0x20
            ]
        );
        assert_eq!(
            &ps[29..45],
            &[
                0x00, 0x00, 0x01, 0xbc, 0x00, 0x0e, 0xc1, 0xff, 0x00, 0x00, 0x00, 0x04, 0x90, 0xc0,
                0x00, 0x00
            ]
        );
        assert_eq!(mpeg2_crc32(&ps[29..49]), 0);
        assert_eq!(
            &ps[49..63],
            &[
                0x00, 0x00, 0x01, 0xc0, 0x00, 0xa8, 0x84, 0x80, 0x05, 0x21, 0x00, 0x01, 0x00, 0x01
            ]
        );
        assert!(ps[63..].iter().all(|byte| *byte == 0xd5));
        assert_eq!(packets[0].sequence, 0x1234);
        assert_eq!(packets[0].timestamp, 0);
        assert!(packets[0].marker);
        assert_eq!(packets[0].bytes[1], 0xe0);
    }

    #[test]
    fn fragments_share_timestamp_and_only_last_packet_has_marker() {
        let mut packetizer = PsRtpPacketizer::new(config(64)).unwrap();
        let first = packetizer.packetize(&[0; 160]).unwrap();
        assert_eq!(first.len(), 4);
        assert_eq!(
            first
                .iter()
                .map(|packet| packet.sequence)
                .collect::<Vec<_>>(),
            vec![0x1234, 0x1235, 0x1236, 0x1237]
        );
        assert!(first.iter().all(|packet| packet.timestamp == 0));
        assert_eq!(
            first.iter().map(|packet| packet.marker).collect::<Vec<_>>(),
            vec![false, false, false, true]
        );
        assert_eq!(
            first
                .iter()
                .map(|packet| payload(packet).len())
                .collect::<Vec<_>>(),
            vec![64, 64, 64, 31]
        );

        let second = packetizer.packetize(&[0; 160]).unwrap();
        assert_eq!(second[0].sequence, 0x1238);
        assert!(second.iter().all(|packet| packet.timestamp == 1_800));
        assert!(
            !payload(&second[0])
                .windows(4)
                .any(|value| value == [0x00, 0x00, 0x01, 0xbb])
        );
        assert!(
            !payload(&second[0])
                .windows(4)
                .any(|value| value == [0x00, 0x00, 0x01, 0xbc])
        );
    }

    #[test]
    fn validates_frame_contract_and_has_idempotent_end() {
        assert!(matches!(
            PsRtpPacketizer::new(PsRtpPacketizerConfig {
                payload_type: 128,
                ..config(1_400)
            }),
            Err(PsPacketizerError::InvalidPayloadType(128))
        ));
        let mut packetizer = PsRtpPacketizer::new(config(1_400)).unwrap();
        assert!(matches!(
            packetizer.packetize(&[0; 159]),
            Err(PsPacketizerError::InvalidFrameLength {
                expected: 160,
                actual: 159
            })
        ));
        packetizer.packetize(&[0; 160]).unwrap();
        let end = packetizer.finish();
        assert_eq!(end.len(), 1);
        assert_eq!(payload(&end[0]), [0x00, 0x00, 0x01, 0xb9]);
        assert!(packetizer.finish().is_empty());
        assert_eq!(
            packetizer.packetize(&[0; 160]),
            Err(PsPacketizerError::Finished)
        );
    }

    #[test]
    fn sequence_and_rtp_timestamp_wrap_without_pts_regression() {
        let mut packetizer = PsRtpPacketizer::new(PsRtpPacketizerConfig {
            sequence: u16::MAX,
            timestamp: u32::MAX - 899,
            max_rtp_payload_len: 128,
            ..config(128)
        })
        .unwrap();
        let first = packetizer.packetize(&[0; 160]).unwrap();
        assert_eq!(first[0].sequence, u16::MAX);
        assert_eq!(first[1].sequence, 0);
        let second = packetizer.packetize(&[0; 160]).unwrap();
        assert_eq!(second[0].timestamp, 900);
    }

    #[test]
    fn ffmpeg_demuxer_recognizes_pcma_audio_from_generated_ps() {
        let mut packetizer = PsRtpPacketizer::new(config(DEFAULT_MAX_RTP_PAYLOAD_LEN)).unwrap();
        let mut ps = Vec::new();
        for _ in 0..100 {
            for packet in packetizer.packetize(&[0xd5; 160]).unwrap() {
                ps.extend_from_slice(payload(&packet));
            }
        }
        for packet in packetizer.finish() {
            ps.extend_from_slice(payload(&packet));
        }

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("gmv-pcma-{unique}.ps"));
        fs::write(&path, &ps).unwrap();
        let c_path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut options = None;
        let result = AVFormatContextInput::open(&c_path, None, &mut options);
        let _ = fs::remove_file(&path);
        let context = result.expect("FFmpeg MPEG-PS demuxer must parse packetizer output");
        let audio = context
            .streams()
            .into_iter()
            .find(|stream| stream.codecpar().codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO)
            .expect("generated PS contains an audio stream");
        let codec = audio.codecpar();
        assert_eq!(codec.codec_id, AVCodecID_AV_CODEC_ID_PCM_ALAW);
        assert_eq!(codec.sample_rate, 8_000);
        assert_eq!(codec.ch_layout.nb_channels, 1);
    }

    #[test]
    fn ten_minutes_and_fifty_legs_stay_bounded() {
        const TEN_MINUTE_FRAME_COUNT: usize = 10 * 60 * 50;
        let frame = [0xd5; 160];
        let started = Instant::now();
        let mut packetizer = PsRtpPacketizer::new(config(DEFAULT_MAX_RTP_PAYLOAD_LEN)).unwrap();
        let initial_capacity = packetizer.ps_buffer_capacity();
        let mut last_timestamp = None;
        for _ in 0..TEN_MINUTE_FRAME_COUNT {
            let packets = packetizer.packetize(&frame).unwrap();
            assert_eq!(packets.len(), 1);
            if let Some(previous) = last_timestamp {
                assert_eq!(packets[0].timestamp.wrapping_sub(previous), 1_800);
            }
            last_timestamp = Some(packets[0].timestamp);
        }
        assert_eq!(packetizer.ps_buffer_capacity(), initial_capacity);
        assert!(started.elapsed() < Duration::from_secs(40));

        let fanout_started = Instant::now();
        let mut legs = (0..MAX_BROADCAST_LEGS_PER_PARENT)
            .map(|index| {
                PsRtpPacketizer::new(PsRtpPacketizerConfig {
                    ssrc: index as u32 + 1,
                    ..config(DEFAULT_MAX_RTP_PAYLOAD_LEN)
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        for _ in 0..50 {
            for leg in &mut legs {
                assert_eq!(leg.packetize(&frame).unwrap().len(), 1);
            }
        }
        assert!(fanout_started.elapsed() < Duration::from_secs(40));
        assert_eq!(MAX_BROADCAST_PARENTS_PER_NODE, 8);
        assert_eq!(MAX_BROADCAST_LEGS_PER_NODE, 50);
    }

    #[test]
    fn benchmark_records_single_fanout_and_multi_parent_profiles() {
        fn run_profile(parent_count: usize, legs_per_parent: usize) -> (Duration, Duration) {
            let mut parents = (0..parent_count)
                .map(|parent| {
                    (0..legs_per_parent)
                        .map(|leg| {
                            PsRtpPacketizer::new(PsRtpPacketizerConfig {
                                ssrc: (parent * legs_per_parent + leg + 1) as u32,
                                ..config(DEFAULT_MAX_RTP_PAYLOAD_LEN)
                            })
                            .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let frame = [0xd5; 160];
            let started = Instant::now();
            let mut peak_tick = Duration::ZERO;
            for _ in 0..3_000 {
                let tick_started = Instant::now();
                for legs in &mut parents {
                    for leg in legs {
                        assert_eq!(leg.packetize(&frame).unwrap().len(), 1);
                    }
                }
                peak_tick = peak_tick.max(tick_started.elapsed());
            }
            (started.elapsed(), peak_tick)
        }

        for (parents, legs) in [(1, 1), (1, 10), (1, 50), (4, 10)] {
            let (elapsed, peak_tick) = run_profile(parents, legs);
            eprintln!(
                "PS packetizer benchmark: parents={parents}, legs_per_parent={legs}, frames=3000, elapsed_us={}, peak_tick_us={}, steady_allocations_per_leg_frame=2, ps_buffer_bytes=512",
                elapsed.as_micros(),
                peak_tick.as_micros()
            );
            assert!(peak_tick < Duration::from_millis(40));
        }
    }
}
