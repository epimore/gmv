use crate::media::context::RtpState;
use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, warn};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use shared::info::media_info_ext::MediaExt;
use std::collections::VecDeque;
use std::ptr;
use std::time::{Duration, Instant};

pub struct RtpPacket {
    pub ssrc: u32,
    pub timestamp: u32,
    pub marker: bool,
    pub seq: u16,
    pub payload: Bytes,
}

const BUFFER_SIZE: usize = 1024;
const DEFAULT_VIDEO_QUEUE_WINDOW: usize = 32;
const DEFAULT_AUDIO_QUEUE_WINDOW: usize = 8;
const MAX_QUEUE_WINDOW: usize = 128;
const MIN_VIDEO_QUEUE_WINDOW: usize = 16;
const MIN_AUDIO_QUEUE_WINDOW: usize = 4;
const QUEUE_SHRINK_AFTER_IN_ORDER: usize = 256;
const REORDER_BUFFER_HIGH_WATERMARK: usize = BUFFER_SIZE * 4 / 5;
const MIN_GAP_WAIT_MS: u64 = 5;
const DEFAULT_GAP_WAIT_MS: u64 = 20;
const MAX_GAP_WAIT_MS: u64 = 60;
const GAP_WAIT_PHASE1_MS: u64 = 5;
const GAP_WAIT_PHASE2_MS: u64 = 10;
const GAP_WAIT_SAFETY_MARGIN_MS: u64 = 8;
const GAP_WAIT_SHRINK_AFTER_IN_ORDER: usize = 512;
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

fn reorder_window(payload_kind: PayloadKind) -> (usize, usize) {
    match payload_kind {
        PayloadKind::Ps | PayloadKind::H264 | PayloadKind::H265 | PayloadKind::Passthrough => {
            (DEFAULT_VIDEO_QUEUE_WINDOW, MIN_VIDEO_QUEUE_WINDOW)
        }
        PayloadKind::Aac | PayloadKind::G711 => {
            (DEFAULT_AUDIO_QUEUE_WINDOW, MIN_AUDIO_QUEUE_WINDOW)
        }
    }
}

fn default_gap_wait_ms(payload_kind: PayloadKind) -> u64 {
    match payload_kind {
        PayloadKind::Ps | PayloadKind::H264 | PayloadKind::H265 | PayloadKind::Passthrough => {
            DEFAULT_GAP_WAIT_MS
        }
        PayloadKind::Aac | PayloadKind::G711 => MIN_GAP_WAIT_MS,
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

fn seq_in_range(seq: u16, start: u16, end: u16) -> bool {
    if start <= end {
        seq >= start && seq <= end
    } else {
        seq >= start || seq <= end
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy)]
struct LateGap {
    start_seq: u16,
    end_seq: u16,
    started_at: Instant,
}

pub struct RtpPacketBuffer {
    pub ssrc: u32,
    first_read_rtp_sn: u16,
    queue: [Option<RtpPacket>; BUFFER_SIZE],
    queue_count: usize,
    queue_window: usize,
    min_queue_window: usize,
    in_order_packets: usize,
    gap_wait_ms: u64,
    gap_in_order_packets: usize,
    late_gap: Option<LateGap>,
    packet_rx: Receiver<RtpPacket>,
    remaining: Bytes,
    ready_aus: VecDeque<Bytes>,
    au_buffer: BytesMut,
    au_timestamp: Option<u32>,
    au_damaged: bool,
    wait_keyframe: bool,
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
        let payload_kind = PayloadKind::from_media_ext(media_ext);
        let (queue_window, min_queue_window) = reorder_window(payload_kind);
        let mut buffer = Self {
            ssrc,
            first_read_rtp_sn: u16::MAX,
            queue: std::array::from_fn(|_| None),
            queue_count: 0,
            queue_window,
            min_queue_window,
            in_order_packets: 0,
            gap_wait_ms: default_gap_wait_ms(payload_kind),
            gap_in_order_packets: 0,
            late_gap: None,
            packet_rx,
            remaining: Default::default(),
            ready_aus: VecDeque::new(),
            au_buffer: BytesMut::new(),
            au_timestamp: None,
            au_damaged: false,
            wait_keyframe: matches!(payload_kind, PayloadKind::H264 | PayloadKind::H265),
            payload_kind,
            h264_fu: None,
            h265_fu: None,
            aac_adts: AacAdtsConfig::from_media_ext(media_ext),
        };
        buffer.calculate_index()?;
        Ok(buffer)
    }

    fn calculate_index(&mut self) -> GlobalResult<()> {
        while self.queue_count < self.queue_window {
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
        if let Some(copy_len) = self.consume_ready_au(max_consume_len, buf) {
            return Some(copy_len);
        }

        loop {
            let input_closed = !self.reduce_packet();
            let Some((pkt, lost_before)) = self.take_next_packet(input_closed) else {
                if input_closed && self.queue_count == 0 {
                    self.finish_access_unit(false);
                    return self.consume_ready_au(max_consume_len, buf);
                }
                return None;
            };
            unsafe {
                (*rtp_state).timestamp = pkt.timestamp;
                (*rtp_state).marker = pkt.marker;
            }

            self.process_packet(pkt, lost_before);
            if let Some(copy_len) = self.consume_ready_au(max_consume_len, buf) {
                return Some(copy_len);
            }

            if input_closed && self.queue_count == 0 {
                self.finish_access_unit(false);
                if let Some(copy_len) = self.consume_ready_au(max_consume_len, buf) {
                    return Some(copy_len);
                }
                return None;
            }
        }
    }

    fn consume_ready_au(&mut self, max_consume_len: usize, buf: *mut u8) -> Option<usize> {
        let data = self.ready_aus.pop_front()?;
        self.remaining = data;
        self.consume_remaining(max_consume_len, buf)
    }

    fn process_packet(&mut self, pkt: RtpPacket, lost_before: bool) {
        match self.payload_kind {
            PayloadKind::Ps | PayloadKind::Passthrough => {
                let timestamp = pkt.timestamp;
                let marker = pkt.marker;
                if lost_before {
                    self.mark_packet_loss(timestamp);
                }
                self.append_access_unit(timestamp, marker, pkt.payload);
            }
            PayloadKind::H264 => {
                let timestamp = pkt.timestamp;
                let marker = pkt.marker;
                if lost_before {
                    self.mark_packet_loss(timestamp);
                }
                if let Some(data) = self.depacketize_h264(pkt.payload.as_ref()) {
                    self.append_access_unit(timestamp, marker, data);
                } else if marker {
                    self.finish_access_unit(true);
                }
            }
            PayloadKind::H265 => {
                let timestamp = pkt.timestamp;
                let marker = pkt.marker;
                if lost_before {
                    self.mark_packet_loss(timestamp);
                }
                if let Some(data) = self.depacketize_h265(pkt.payload.as_ref()) {
                    self.append_access_unit(timestamp, marker, data);
                } else if marker {
                    self.finish_access_unit(true);
                }
            }
            PayloadKind::Aac => {
                if lost_before {
                    warn!(
                        "aac rtp sequence loss; keep next intact packet; ssrc: {}, timestamp: {}",
                        self.ssrc, pkt.timestamp
                    );
                }
                if let Some(data) = self.depacketize_aac(pkt.payload) {
                    self.ready_aus.push_back(data);
                }
            }
            PayloadKind::G711 => {
                if lost_before {
                    warn!(
                        "g711 rtp sequence loss; keep next intact packet; ssrc: {}, timestamp: {}",
                        self.ssrc, pkt.timestamp
                    );
                }
                self.ready_aus.push_back(pkt.payload);
            }
        }
    }

    fn append_access_unit(&mut self, timestamp: u32, marker: bool, data: Bytes) {
        if self
            .au_timestamp
            .is_some_and(|current| current != timestamp)
        {
            self.finish_access_unit(false);
        }

        if self.au_timestamp.is_none() {
            self.au_timestamp = Some(timestamp);
        }
        self.au_buffer.extend_from_slice(data.as_ref());

        if marker {
            self.finish_access_unit(true);
        }
    }

    fn finish_access_unit(&mut self, marker: bool) {
        if self.au_buffer.is_empty() {
            self.au_timestamp = None;
            self.au_damaged = false;
            return;
        }

        if self.au_damaged {
            warn!(
                "drop damaged rtp access unit; ssrc: {}, timestamp: {:?}, bytes: {}",
                self.ssrc,
                self.au_timestamp,
                self.au_buffer.len()
            );
            self.au_buffer.clear();
            if self.is_raw_video() {
                self.wait_keyframe = true;
            }
        } else {
            let data = self.au_buffer.split().freeze();
            if self.should_output_access_unit(data.as_ref()) {
                self.ready_aus.push_back(data);
            }
        }

        self.au_timestamp = None;
        self.au_damaged = false;
    }

    fn should_output_access_unit(&mut self, data: &[u8]) -> bool {
        if !self.is_raw_video() {
            return true;
        }

        let keyframe = match self.payload_kind {
            PayloadKind::H264 => h264_access_unit_has_keyframe(data),
            PayloadKind::H265 => h265_access_unit_has_keyframe(data),
            _ => false,
        };

        if self.wait_keyframe {
            if keyframe {
                self.wait_keyframe = false;
                true
            } else {
                debug!(
                    "drop video access unit while waiting keyframe; ssrc: {}",
                    self.ssrc
                );
                false
            }
        } else {
            true
        }
    }

    fn mark_packet_loss(&mut self, next_timestamp: u32) {
        let had_fragment = self.h264_fu.is_some() || self.h265_fu.is_some();
        self.reset_fragment_state();

        if self.au_buffer.is_empty() {
            self.au_timestamp = None;
            self.au_damaged = false;
            if had_fragment && self.is_raw_video() {
                self.wait_keyframe = true;
            }
            return;
        }

        if self
            .au_timestamp
            .is_some_and(|current| current != next_timestamp)
        {
            warn!(
                "drop rtp access unit before sequence loss; ssrc: {}, timestamp: {:?}, bytes: {}",
                self.ssrc,
                self.au_timestamp,
                self.au_buffer.len()
            );
            self.au_buffer.clear();
            self.au_timestamp = None;
            self.au_damaged = false;
            return;
        }

        self.au_buffer.clear();
        self.au_timestamp = Some(next_timestamp);
        self.au_damaged = true;
    }

    fn is_raw_video(&self) -> bool {
        matches!(self.payload_kind, PayloadKind::H264 | PayloadKind::H265)
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
            self.record_old_packet(seq);
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

    fn take_next_packet(&mut self, input_closed: bool) -> Option<(RtpPacket, bool)> {
        if self.queue_count == 0 {
            return None;
        }

        if let Some(pkt) = self.take_expected_packet() {
            self.first_read_rtp_sn = pkt.seq.wrapping_add(1);
            if !input_closed {
                self.adjust_queue_window(0);
                self.adjust_gap_wait_after_in_order();
            }
            return Some((pkt, false));
        }

        let gap_started_at = Instant::now();
        if !input_closed && self.wait_for_gap_packet(self.first_read_rtp_sn, gap_started_at) {
            if let Some(pkt) = self.take_expected_packet() {
                self.record_late_packet(elapsed_ms(gap_started_at));
                self.in_order_packets = 0;
                self.first_read_rtp_sn = pkt.seq.wrapping_add(1);
                return Some((pkt, false));
            }
        }

        let Some((pkt, offset)) = self.take_first_available_packet() else {
            return None;
        };

        let lost_before = offset > 0;
        if lost_before {
            debug!(
                "rtp packet lost; ssrc: {}, expected seq: {}, next seq: {}, missed: {}, wait_ms: {}, queue_count: {}",
                self.ssrc,
                self.first_read_rtp_sn,
                pkt.seq,
                offset,
                self.gap_wait_ms,
                self.queue_count
            );
            if !input_closed {
                self.remember_late_gap(
                    self.first_read_rtp_sn,
                    pkt.seq.wrapping_sub(1),
                    gap_started_at,
                );
            }
            self.gap_in_order_packets = 0;
        }

        self.first_read_rtp_sn = pkt.seq.wrapping_add(1);
        if !input_closed {
            self.adjust_queue_window(offset);
        }

        Some((pkt, lost_before))
    }

    fn take_expected_packet(&mut self) -> Option<RtpPacket> {
        let seq = self.first_read_rtp_sn;
        let index = seq as usize % BUFFER_SIZE;
        let item = unsafe { self.queue.get_unchecked_mut(index) };
        if !item.as_ref().map(|pkt| pkt.seq == seq).unwrap_or(false) {
            return None;
        }

        self.queue_count -= 1;
        item.take()
    }

    fn take_first_available_packet(&mut self) -> Option<(RtpPacket, usize)> {
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
            return Some((pkt, offset));
        }
        None
    }

    fn has_packet(&self, seq: u16) -> bool {
        let index = seq as usize % BUFFER_SIZE;
        self.queue[index]
            .as_ref()
            .map(|pkt| pkt.seq == seq)
            .unwrap_or(false)
    }

    fn wait_for_gap_packet(&mut self, expected_seq: u16, gap_started_at: Instant) -> bool {
        let max_wait_ms = self
            .gap_wait_ms
            .clamp(self.min_gap_wait_ms(), MAX_GAP_WAIT_MS);
        let phase2_ms = GAP_WAIT_PHASE1_MS + GAP_WAIT_PHASE2_MS;
        let phases = [
            GAP_WAIT_PHASE1_MS.min(max_wait_ms),
            phase2_ms.min(max_wait_ms),
            max_wait_ms,
        ];

        let mut last_phase_ms = 0;
        for phase_ms in phases {
            if phase_ms <= last_phase_ms {
                continue;
            }
            if self.wait_until_gap_deadline(expected_seq, gap_started_at, phase_ms) {
                return true;
            }
            last_phase_ms = phase_ms;
        }
        false
    }

    fn wait_until_gap_deadline(
        &mut self,
        expected_seq: u16,
        gap_started_at: Instant,
        phase_ms: u64,
    ) -> bool {
        let deadline = gap_started_at + Duration::from_millis(phase_ms);
        loop {
            if self.has_packet(expected_seq) {
                return true;
            }
            if self.queue_count >= REORDER_BUFFER_HIGH_WATERMARK {
                debug!(
                    "rtp reorder buffer high watermark; ssrc: {}, expected seq: {}, queue_count: {}, wait_ms: {}",
                    self.ssrc, expected_seq, self.queue_count, self.gap_wait_ms
                );
                return false;
            }

            let now = Instant::now();
            if now >= deadline {
                return false;
            }

            match self.packet_rx.recv_timeout(deadline.duration_since(now)) {
                Ok(pkt) => self.enqueue(pkt),
                Err(RecvTimeoutError::Timeout) => return self.has_packet(expected_seq),
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    fn remember_late_gap(&mut self, start_seq: u16, end_seq: u16, started_at: Instant) {
        self.late_gap = Some(LateGap {
            start_seq,
            end_seq,
            started_at,
        });
    }

    fn record_old_packet(&mut self, seq: u16) {
        let Some(gap) = self.late_gap else {
            return;
        };
        if !seq_in_range(seq, gap.start_seq, gap.end_seq) {
            return;
        }

        self.record_late_packet(elapsed_ms(gap.started_at));
        if seq == gap.end_seq {
            self.late_gap = None;
        }
    }

    fn record_late_packet(&mut self, late_ms: u64) {
        self.gap_in_order_packets = 0;
        let target = late_ms
            .saturating_add(GAP_WAIT_SAFETY_MARGIN_MS)
            .clamp(self.min_gap_wait_ms(), MAX_GAP_WAIT_MS);
        if target <= self.gap_wait_ms {
            return;
        }

        self.gap_wait_ms = target;
        debug!(
            "increase rtp gap wait; ssrc: {}, late_ms: {}, wait_ms: {}",
            self.ssrc, late_ms, self.gap_wait_ms
        );
    }

    fn adjust_gap_wait_after_in_order(&mut self) {
        let min_gap_wait_ms = self.min_gap_wait_ms();
        if self.gap_wait_ms <= min_gap_wait_ms {
            return;
        }

        self.gap_in_order_packets += 1;
        if self.gap_in_order_packets < GAP_WAIT_SHRINK_AFTER_IN_ORDER {
            return;
        }

        self.gap_wait_ms = self.gap_wait_ms.saturating_sub(1).max(min_gap_wait_ms);
        self.gap_in_order_packets = 0;
        debug!(
            "decrease rtp gap wait; ssrc: {}, wait_ms: {}",
            self.ssrc, self.gap_wait_ms
        );
    }

    fn min_gap_wait_ms(&self) -> u64 {
        default_gap_wait_ms(self.payload_kind)
    }

    fn adjust_queue_window(&mut self, offset: usize) {
        if offset > 0 {
            self.in_order_packets = 0;
            let target = (offset + self.min_queue_window).min(MAX_QUEUE_WINDOW);
            if target > self.queue_window {
                self.queue_window = target;
            } else if offset + 2 >= self.queue_window && self.queue_window < MAX_QUEUE_WINDOW {
                self.queue_window += 1;
            }
            return;
        }

        if self.queue_window > self.min_queue_window {
            self.in_order_packets += 1;
            if self.in_order_packets >= QUEUE_SHRINK_AFTER_IN_ORDER {
                self.queue_window -= 1;
                self.in_order_packets = 0;
            }
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

fn h264_access_unit_has_keyframe(data: &[u8]) -> bool {
    access_unit_has_nal(data, |nal| (nal[0] & 0x1f) == 5)
}

fn h265_access_unit_has_keyframe(data: &[u8]) -> bool {
    access_unit_has_nal(data, |nal| {
        if nal.len() < 2 {
            return false;
        }
        matches!((nal[0] >> 1) & 0x3f, 19 | 20 | 21)
    })
}

fn access_unit_has_nal<F>(data: &[u8], mut predicate: F) -> bool
where
    F: FnMut(&[u8]) -> bool,
{
    let Some((first_start, first_start_len)) = find_start_code(data, 0) else {
        return !data.is_empty() && predicate(data);
    };

    let mut nal_start = first_start + first_start_len;
    while nal_start < data.len() {
        match find_start_code(data, nal_start) {
            Some((next_start, next_start_len)) => {
                if next_start > nal_start && predicate(&data[nal_start..next_start]) {
                    return true;
                }
                nal_start = next_start + next_start_len;
            }
            None => return predicate(&data[nal_start..]),
        }
    }
    false
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    while pos + 3 <= data.len() {
        if pos + 4 <= data.len() && data[pos..pos + 4] == [0, 0, 0, 1] {
            return Some((pos, 4));
        }
        if data[pos..pos + 3] == [0, 0, 1] {
            return Some((pos, 3));
        }
        pos += 1;
    }
    None
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
