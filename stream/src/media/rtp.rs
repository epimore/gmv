use crate::media::context::RtpState;
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, warn};
use crossbeam_channel::Receiver;
use shared::info::media_info_ext::MediaExt;
use std::ptr;

pub struct RtpPacket {
    pub ssrc: u32,
    pub timestamp: u32,
    pub marker: bool,
    pub seq: u16,
    pub payload: Bytes,
}

const BUFFER_SIZE: usize = 64;
const DEFAULT_QUEUE_WINDOW: usize = 8;
const MAX_QUEUE_WINDOW: usize = BUFFER_SIZE / 2;
const MIN_QUEUE_WINDOW: usize = 4;
const SEQ_HALF_RANGE: u16 = 32768;
const START_CODE: &[u8; 4] = &[0, 0, 0, 1];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadKind {
    Ps,
    H264,
    H265,
    Aac,
    G711,
    Passthrough,
}

impl PayloadKind {
    fn from_media_ext(media_ext: &MediaExt) -> Self {
        let type_name = media_ext.type_name.to_ascii_uppercase();
        match type_name.as_str() {
            "PS" | "MP2P" | "MP2PS" => Self::Ps,
            "H264" | "H.264" | "AVC" => Self::H264,
            "H265" | "H.265" | "HEVC" => Self::H265,
            "AAC" | "MPEG4-GENERIC" => Self::Aac,
            "G711" | "G711A" | "G711U" | "PCMA" | "PCMU" => Self::G711,
            _ => {
                if matches_codec(&media_ext.video_params.codec_id, &["h264", "h.264", "avc"]) {
                    Self::H264
                } else if matches_codec(
                    &media_ext.video_params.codec_id,
                    &["h265", "h.265", "hevc"],
                ) {
                    Self::H265
                } else if matches_codec(&media_ext.audio_params.codec_id, &["aac"]) {
                    Self::Aac
                } else if matches_codec(
                    &media_ext.audio_params.codec_id,
                    &[
                        "g711", "g.711", "g711a", "g.711a", "g711u", "g.711u", "pcma", "pcmu",
                        "alaw", "mulaw", "ulaw",
                    ],
                ) {
                    Self::G711
                } else {
                    Self::Passthrough
                }
            }
        }
    }
}

fn matches_codec(codec: &Option<String>, candidates: &[&str]) -> bool {
    codec
        .as_deref()
        .map(|s| {
            let lower = s.to_ascii_lowercase();
            candidates.iter().any(|candidate| lower == *candidate)
        })
        .unwrap_or(false)
}

#[derive(Clone, Copy)]
struct AacAdtsConfig {
    sample_rate: usize,
    channels: usize,
}

impl AacAdtsConfig {
    fn from_media_ext(media_ext: &MediaExt) -> Self {
        let sample_rate = media_ext
            .audio_params
            .sample_rate
            .as_deref()
            .and_then(parse_sample_rate)
            .or_else(|| {
                if media_ext.audio_params.clock_rate > 0 {
                    Some(media_ext.audio_params.clock_rate as usize)
                } else if media_ext.clock_rate > 0 {
                    Some(media_ext.clock_rate as usize)
                } else {
                    None
                }
            })
            .unwrap_or(8000);
        let channels = media_ext.audio_params.channel_count.max(1) as usize;
        Self {
            sample_rate,
            channels,
        }
    }
}

fn parse_sample_rate(s: &str) -> Option<usize> {
    let rate = s.parse::<f64>().ok()?;
    if rate <= 0.0 {
        return None;
    }
    if rate < 1000.0 {
        Some((rate * 1000.0).round() as usize)
    } else {
        Some(rate.round() as usize)
    }
}

fn seq_before(seq: u16, base: u16) -> bool {
    seq != base && base.wrapping_sub(seq) < SEQ_HALF_RANGE
}

pub struct RtpPacketBuffer {
    pub ssrc: u32,
    first_read_rtp_sn: u16,
    queue: [Option<RtpPacket>; BUFFER_SIZE],
    queue_count: usize,
    queue_window: usize,
    packet_rx: Receiver<RtpPacket>,
    remaining: Bytes,
    payload_kind: PayloadKind,
    h264_fu: Option<BytesMut>,
    h265_fu: Option<BytesMut>,
    aac_adts: AacAdtsConfig,
}

impl RtpPacketBuffer {
    pub fn init(
        ssrc: u32,
        packet_rx: Receiver<RtpPacket>,
        media_ext: &MediaExt,
    ) -> GlobalResult<Self> {
        let mut buffer = Self {
            ssrc,
            first_read_rtp_sn: u16::MAX,
            queue: std::array::from_fn(|_| None),
            queue_count: 0,
            queue_window: DEFAULT_QUEUE_WINDOW,
            packet_rx,
            remaining: Default::default(),
            payload_kind: PayloadKind::from_media_ext(media_ext),
            h264_fu: None,
            h265_fu: None,
            aac_adts: AacAdtsConfig::from_media_ext(media_ext),
        };
        buffer.calculate_index()?;
        Ok(buffer)
    }

    fn calculate_index(&mut self) -> GlobalResult<()> {
        while self.queue_count < DEFAULT_QUEUE_WINDOW {
            let pkt = self.recv_packet()?;
            self.enqueue_initial(pkt);
        }
        Ok(())
    }

    pub fn consume_packet(
        &mut self,
        max_consume_len: usize,
        buf: *mut u8,
        rtp_state: *mut RtpState,
    ) -> Option<usize> {
        if max_consume_len == 0 {
            return Some(0);
        }

        if let Some(copy_len) = self.consume_remaining(max_consume_len, buf) {
            return Some(copy_len);
        }

        loop {
            let input_closed = !self.reduce_packet();
            let Some(pkt) = self.take_next_packet(input_closed) else {
                return None;
            };
            unsafe {
                (*rtp_state).timestamp = pkt.timestamp;
                (*rtp_state).marker = pkt.marker;
            }

            if let Some(data) = self.depacketize(pkt) {
                self.remaining = data;
                return self.consume_remaining(max_consume_len, buf);
            }

            if input_closed && self.queue_count == 0 {
                return None;
            }
        }
    }

    fn reduce_packet(&mut self) -> bool {
        while self.queue_count < self.queue_window {
            let Ok(pkt) = self.recv_packet() else {
                return false;
            };
            self.enqueue(pkt);
        }
        true
    }

    fn recv_packet(&self) -> GlobalResult<RtpPacket> {
        self.packet_rx
            .recv()
            .map_err(|_| GlobalError::new_sys_error("rtp input channel closed", |_| {}))
    }

    fn enqueue_initial(&mut self, pkt: RtpPacket) {
        let seq = pkt.seq;
        let index = seq as usize % BUFFER_SIZE;
        let item = unsafe { self.queue.get_unchecked_mut(index) };
        if item.as_ref().map(|pkt| pkt.seq == seq).unwrap_or(false) {
            return;
        }

        if self.queue_count == 0 || seq_before(seq, self.first_read_rtp_sn) {
            self.first_read_rtp_sn = seq;
        }

        if item.is_none() {
            self.queue_count += 1;
        }
        *item = Some(pkt);
    }

    fn enqueue(&mut self, pkt: RtpPacket) {
        let seq = pkt.seq;
        if self.is_old_packet(seq) {
            debug!(
                "drop old rtp packet; ssrc: {}, seq: {}, first read seq: {}",
                self.ssrc, seq, self.first_read_rtp_sn
            );
            return;
        }

        let index = seq as usize % BUFFER_SIZE;
        let item = unsafe { self.queue.get_unchecked_mut(index) };
        if item.as_ref().map(|pkt| pkt.seq == seq).unwrap_or(false) {
            return;
        }
        if item.is_none() {
            self.queue_count += 1;
        } else if let Some(existing) = item.as_ref() {
            debug!(
                "replace rtp queue slot; ssrc: {}, old seq: {}, new seq: {}",
                self.ssrc, existing.seq, seq
            );
        }
        *item = Some(pkt);
    }

    fn is_old_packet(&self, seq: u16) -> bool {
        seq != self.first_read_rtp_sn && self.first_read_rtp_sn.wrapping_sub(seq) < SEQ_HALF_RANGE
    }

    fn take_next_packet(&mut self, input_closed: bool) -> Option<RtpPacket> {
        if self.queue_count == 0 {
            return None;
        }

        let mut index = self.first_read_rtp_sn as usize % BUFFER_SIZE;
        for offset in 0..BUFFER_SIZE {
            if index == BUFFER_SIZE {
                index = 0;
            }
            let item = unsafe { self.queue.get_unchecked_mut(index) };
            index += 1;

            let Some(pkt) = item.take() else {
                continue;
            };

            self.queue_count -= 1;
            if offset > 0 {
                debug!(
                    "rtp packet lost; ssrc: {}, expected seq: {}, next seq: {}, missed: {}",
                    self.ssrc, self.first_read_rtp_sn, pkt.seq, offset
                );
                self.reset_fragment_state();
            }

            self.first_read_rtp_sn = pkt.seq.wrapping_add(1);
            if !input_closed {
                self.adjust_queue_window(offset);
            }

            return Some(pkt);
        }
        None
    }

    fn adjust_queue_window(&mut self, offset: usize) {
        if offset > self.queue_window && self.queue_window < MAX_QUEUE_WINDOW {
            self.queue_window += 1;
        } else if offset == 0 && self.queue_window > MIN_QUEUE_WINDOW {
            self.queue_window -= 1;
        }
    }

    fn consume_remaining(&mut self, max_consume_len: usize, buf: *mut u8) -> Option<usize> {
        if self.remaining.is_empty() {
            return None;
        }

        let data = std::mem::take(&mut self.remaining);
        let copy_len = data.len().min(max_consume_len);
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), buf, copy_len);
        }
        self.remaining = data.slice(copy_len..);
        Some(copy_len)
    }

    fn depacketize(&mut self, pkt: RtpPacket) -> Option<Bytes> {
        match self.payload_kind {
            PayloadKind::Ps | PayloadKind::G711 | PayloadKind::Passthrough => Some(pkt.payload),
            PayloadKind::Aac => self.depacketize_aac(pkt.payload),
            PayloadKind::H264 => self.depacketize_h264(pkt.payload.as_ref()),
            PayloadKind::H265 => self.depacketize_h265(pkt.payload.as_ref()),
        }
    }

    fn reset_fragment_state(&mut self) {
        self.h264_fu = None;
        self.h265_fu = None;
    }

    fn depacketize_h264(&mut self, payload: &[u8]) -> Option<Bytes> {
        let nal = *payload.first()?;
        let nal_type = nal & 0x1f;
        match nal_type {
            0..=23 => Some(with_start_code(payload)),
            24 => depacketize_h264_stap_a(payload),
            28 => self.depacketize_h264_fu_a(payload),
            _ => {
                warn!("unsupported h264 rtp nal type: {}", nal_type);
                None
            }
        }
    }

    fn depacketize_h264_fu_a(&mut self, payload: &[u8]) -> Option<Bytes> {
        if payload.len() < 3 {
            warn!("short h264 fu-a packet");
            self.h264_fu = None;
            return None;
        }

        let fu_indicator = payload[0];
        let fu_header = payload[1];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        if start && end {
            warn!("invalid h264 fu-a packet with both start and end bits");
            self.h264_fu = None;
            return None;
        }

        if start {
            let reconstructed_nal = (fu_indicator & 0xe0) | (fu_header & 0x1f);
            let mut out = BytesMut::with_capacity(payload.len() + START_CODE.len());
            out.extend_from_slice(START_CODE);
            out.extend_from_slice(&[reconstructed_nal]);
            out.extend_from_slice(&payload[2..]);
            self.h264_fu = Some(out);
            return None;
        }

        let Some(out) = self.h264_fu.as_mut() else {
            warn!("drop h264 fu-a fragment without start");
            return None;
        };
        out.extend_from_slice(&payload[2..]);
        if end {
            return self.h264_fu.take().map(BytesMut::freeze);
        }
        None
    }

    fn depacketize_h265(&mut self, payload: &[u8]) -> Option<Bytes> {
        if payload.len() < 3 {
            warn!("short h265 rtp packet");
            return None;
        }

        let nal_type = (payload[0] >> 1) & 0x3f;
        let layer_id = ((payload[0] << 5) & 0x20) | ((payload[1] >> 3) & 0x1f);
        let tid = payload[1] & 0x07;
        if layer_id != 0 {
            warn!("unsupported multi-layer h265 rtp packet");
            return None;
        }
        if tid == 0 {
            warn!("invalid h265 rtp temporal id");
            return None;
        }
        if nal_type > 50 {
            warn!("unsupported h265 rtp nal type: {}", nal_type);
            return None;
        }
        match nal_type {
            0..=47 => Some(with_start_code(payload)),
            48 => depacketize_h265_ap(payload),
            49 => self.depacketize_h265_fu(payload),
            _ => {
                warn!("unsupported h265 rtp nal type: {}", nal_type);
                None
            }
        }
    }

    fn depacketize_h265_fu(&mut self, payload: &[u8]) -> Option<Bytes> {
        if payload.len() < 4 {
            warn!("short h265 fu packet");
            self.h265_fu = None;
            return None;
        }

        let fu_header = payload[2];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        let fu_type = fu_header & 0x3f;
        if start && end {
            warn!("invalid h265 fu packet with both start and end bits");
            self.h265_fu = None;
            return None;
        }

        if start {
            let new_header = [(payload[0] & 0x81) | (fu_type << 1), payload[1]];
            let mut out = BytesMut::with_capacity(payload.len() + START_CODE.len());
            out.extend_from_slice(START_CODE);
            out.extend_from_slice(&new_header);
            out.extend_from_slice(&payload[3..]);
            self.h265_fu = Some(out);
            return None;
        }

        let Some(out) = self.h265_fu.as_mut() else {
            warn!("drop h265 fu fragment without start");
            return None;
        };
        out.extend_from_slice(&payload[3..]);
        if end {
            return self.h265_fu.take().map(BytesMut::freeze);
        }
        None
    }

    fn depacketize_aac(&self, payload: Bytes) -> Option<Bytes> {
        if is_adts(payload.as_ref()) {
            return Some(payload);
        }

        let payload = payload.as_ref();
        if payload.len() < 4 {
            return None;
        }

        let au_header_bits = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        let au_header_bytes = au_header_bits.div_ceil(8);
        let header_size_bits = 16;
        if au_header_bits == 0
            || au_header_bits % header_size_bits != 0
            || payload.len() < 2 + au_header_bytes
        {
            warn!("unsupported aac rtp payload without ADTS");
            return None;
        }

        let au_count = au_header_bits / header_size_bits;
        let mut data_offset = 2 + au_header_bytes;
        let mut out = BytesMut::with_capacity(payload.len() + au_count * 7);
        for i in 0..au_count {
            let bit_offset = i * header_size_bits;
            let byte_offset = 2 + bit_offset / 8;
            if byte_offset + 2 > payload.len() {
                return None;
            }
            let au_header = u16::from_be_bytes([payload[byte_offset], payload[byte_offset + 1]]);
            let au_size = (au_header >> 3) as usize;
            if data_offset + au_size > payload.len() {
                warn!("aac rtp AU size exceeds payload");
                return None;
            }

            append_adts_frame(
                &mut out,
                &payload[data_offset..data_offset + au_size],
                self.aac_adts,
            );
            data_offset += au_size;
        }

        if out.is_empty() {
            None
        } else {
            Some(out.freeze())
        }
    }
}

fn with_start_code(payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(START_CODE.len() + payload.len());
    out.extend_from_slice(START_CODE);
    out.extend_from_slice(payload);
    out.freeze()
}

fn depacketize_h264_stap_a(payload: &[u8]) -> Option<Bytes> {
    if payload.len() <= 1 {
        return None;
    }

    let mut pos = 1;
    let mut out = BytesMut::with_capacity(payload.len() + START_CODE.len());
    while pos + 2 <= payload.len() {
        let nalu_len = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
        pos += 2;
        if nalu_len == 0 || pos + nalu_len > payload.len() {
            warn!("invalid h264 stap-a nalu size");
            return None;
        }
        out.extend_from_slice(START_CODE);
        out.extend_from_slice(&payload[pos..pos + nalu_len]);
        pos += nalu_len;
    }

    if out.is_empty() {
        None
    } else {
        Some(out.freeze())
    }
}

fn depacketize_h265_ap(payload: &[u8]) -> Option<Bytes> {
    if payload.len() <= 2 {
        return None;
    }

    let mut pos = 2;
    let mut out = BytesMut::with_capacity(payload.len() + START_CODE.len());
    while pos + 2 <= payload.len() {
        let nalu_len = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
        pos += 2;
        if nalu_len == 0 || pos + nalu_len > payload.len() {
            warn!("invalid h265 aggregation nalu size");
            return None;
        }
        out.extend_from_slice(START_CODE);
        out.extend_from_slice(&payload[pos..pos + nalu_len]);
        pos += nalu_len;
    }

    if out.is_empty() {
        None
    } else {
        Some(out.freeze())
    }
}

fn is_adts(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xff && (data[1] & 0xf0) == 0xf0
}

fn append_adts_frame(out: &mut BytesMut, frame: &[u8], cfg: AacAdtsConfig) {
    let Some(sample_rate_index) = aac_sample_rate_index(cfg.sample_rate) else {
        warn!("unsupported aac sample rate for ADTS: {}", cfg.sample_rate);
        return;
    };

    let channels = cfg.channels.min(7);
    let frame_len = frame.len() + 7;
    let profile = 1usize; // AAC LC in ADTS profile field.

    out.extend_from_slice(&[
        0xff,
        0xf1,
        ((profile & 0x03) << 6 | (sample_rate_index & 0x0f) << 2 | (channels >> 2)) as u8,
        (((channels & 0x03) << 6) | ((frame_len >> 11) & 0x03)) as u8,
        ((frame_len >> 3) & 0xff) as u8,
        (((frame_len & 0x07) << 5) | 0x1f) as u8,
        0xfc,
    ]);
    out.extend_from_slice(frame);
}

fn aac_sample_rate_index(sample_rate: usize) -> Option<usize> {
    match sample_rate {
        96000 => Some(0),
        88200 => Some(1),
        64000 => Some(2),
        48000 => Some(3),
        44100 => Some(4),
        32000 => Some(5),
        24000 => Some(6),
        22050 => Some(7),
        16000 => Some(8),
        12000 => Some(9),
        11025 => Some(10),
        8000 => Some(11),
        7350 => Some(12),
        _ => None,
    }
}
