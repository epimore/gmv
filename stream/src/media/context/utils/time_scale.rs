use log::{info, warn};
use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_NOPTS_VALUE, AV_TIME_BASE_Q, AVCodecID, AVMediaType, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVPacket, av_rescale_q,
};

const MAX_JUMP_US: i64 = 5_000_000; // 5s
const DEFAULT_AUDIO_DELTA: i64 = 1024;
const DEFAULT_VIDEO_DELTA: i64 = 1;
const MAX_REORDER_DELAY: i32 = 16;

pub fn repair_missing_timestamps(pkt: &mut AVPacket, reorder_delay: i32) -> bool {
    if pkt.pts == AV_NOPTS_VALUE && pkt.dts == AV_NOPTS_VALUE {
        return false;
    }

    let no_reorder = reorder_delay <= 0;
    if no_reorder && pkt.pts == AV_NOPTS_VALUE {
        pkt.pts = pkt.dts;
    } else if no_reorder && pkt.dts == AV_NOPTS_VALUE {
        pkt.dts = pkt.pts;
    }

    pkt.pts != AV_NOPTS_VALUE && pkt.dts != AV_NOPTS_VALUE
}
const MAX_DELTA_TICKS: i64 = 500_000; // 最大允许的 delta

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessResult {
    Ok,
    Discontinuity,
}

// ============================
// 单流时间线（核心）
// ============================

pub struct StreamTimeline {
    last_dts: i64,        //修正后
    last_pts: i64,        //修正后
    last_origin_dts: i64, //原pkt值，
    normal_delta: i64,
    initialized: bool,
    stream_type: AVMediaType,
    time_base: AVRational,
    codec_id: AVCodecID,
    reorder_delay: i32,
}

impl StreamTimeline {
    pub fn new(
        stream_type: AVMediaType,
        time_base: AVRational,
        codec_id: AVCodecID,
        reorder_delay: i32,
    ) -> Self {
        Self {
            last_dts: 0,
            last_pts: 0,
            last_origin_dts: 0,
            normal_delta: 0,
            initialized: false,
            stream_type,
            time_base,
            codec_id,
            reorder_delay: reorder_delay.clamp(0, MAX_REORDER_DELAY),
        }
    }

    #[inline]
    fn default_delta(&self) -> i64 {
        if self.stream_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
            DEFAULT_AUDIO_DELTA
        } else {
            DEFAULT_VIDEO_DELTA
        }
    }

    fn get_delta(&self) -> i64 {
        if self.normal_delta > 0 {
            self.normal_delta
        } else {
            self.default_delta()
        }
    }

    pub fn process(&mut self, pkt: &mut AVPacket, ssrc: u32) -> ProcessResult {
        if !repair_missing_timestamps(pkt, self.reorder_delay) {
            warn!("Discard packet without pts/dts; ssrc: {}", ssrc);
            return ProcessResult::Ok;
        }

        // ===== 初始化 =====
        if !self.initialized {
            self.last_dts = pkt.dts;
            self.last_pts = pkt.pts;
            self.last_origin_dts = pkt.dts;
            self.initialized = true;
            return ProcessResult::Ok;
        }

        // ===== discontinuity 检测 =====
        // 1. 首次跳变检测
        let mut result = ProcessResult::Ok;
        let dis_dts_diff = pkt.dts - self.last_origin_dts;
        let dis_dts_diff_us = unsafe { av_rescale_q(dis_dts_diff, self.time_base, AV_TIME_BASE_Q) };
        if dis_dts_diff < 0 || dis_dts_diff_us > MAX_JUMP_US {
            info!(
                "ssrc: {} ,Discontinuity: current dts: {}, last dts: {}",
                ssrc, pkt.dts, self.last_origin_dts
            );
            result = ProcessResult::Discontinuity;
        }
        self.last_origin_dts = pkt.dts;
        // 2. 修正数据
        let fix_dts_diff = pkt.dts - self.last_dts;
        let fix_dts_diff_us = unsafe { av_rescale_q(fix_dts_diff, self.time_base, AV_TIME_BASE_Q) };
        if fix_dts_diff < 0 || fix_dts_diff_us > MAX_JUMP_US {
            // 强制单调递增
            let delta = self.get_delta();
            pkt.dts = self.last_dts + delta;
            pkt.pts = pkt.dts;
            self.normal_delta = 0;
        }

        // ===== PTS 修复 =====
        if pkt.pts < pkt.dts {
            pkt.pts = pkt.dts;
        }

        // ===== 学习 delta =====
        let delta = pkt.dts - self.last_dts;
        if delta > 0 && delta < MAX_DELTA_TICKS {
            self.normal_delta = if self.normal_delta == 0 {
                delta
            } else {
                (self.normal_delta * 7 + delta * 3) / 10
            };
        }

        self.last_dts = pkt.dts;
        self.last_pts = pkt.pts;

        result
    }
}

// ============================
// 全局同步
// ============================

pub struct TimelineNormalizer {
    streams: Vec<Option<StreamTimeline>>,
    pub global_base_us: i64,
}

impl TimelineNormalizer {
    pub fn new(n: usize) -> Self {
        Self {
            streams: (0..n).map(|_| None).collect(),
            global_base_us: i64::MAX,
        }
    }

    pub fn init_stream(
        &mut self,
        idx: usize,
        m_tp: AVMediaType,
        time_base: AVRational,
        codec_id: AVCodecID,
        reorder_delay: i32,
    ) {
        if idx >= self.streams.len() {
            self.streams.resize_with(idx + 1, || None);
        }
        self.streams[idx] = Some(StreamTimeline::new(
            m_tp,
            time_base,
            codec_id,
            reorder_delay,
        ));
    }

    pub(in crate::media::context) fn is_stream_initialized(&self, idx: usize) -> bool {
        self.streams.get(idx).is_some_and(Option::is_some)
    }

    pub fn rescale_global_base_us(&mut self, idx: usize, pts: i64) {
        if pts == AV_NOPTS_VALUE {
            return;
        }
        if let Some(Some(stream)) = self.streams.get(idx) {
            let pts_us = unsafe { av_rescale_q(pts, stream.time_base, AV_TIME_BASE_Q) };
            self.global_base_us = self.global_base_us.min(pts_us);
        }
    }

    pub fn process(&mut self, pkt: &mut AVPacket, ssrc: u32) -> (Option<i64>, ProcessResult) {
        if pkt.stream_index < 0 {
            warn!(
                "Discard packet with invalid stream index; ssrc: {}, stream_index: {}",
                ssrc, pkt.stream_index
            );
            return (None, ProcessResult::Ok);
        }

        let idx = pkt.stream_index as usize;
        if idx >= self.streams.len() {
            warn!(
                "Discard packet with out-of-range stream index; ssrc: {}, stream_index: {}",
                ssrc, pkt.stream_index
            );
            return (None, ProcessResult::Ok);
        }

        if pkt.data.is_null() || pkt.size <= 0 {
            warn!(
                "Discard empty packet; ssrc: {}, pts: {}, dts: {}",
                ssrc, pkt.pts, pkt.dts,
            );
            return (None, ProcessResult::Ok);
        }

        let global_base_us = self.global_base_us;
        let stream = match &mut self.streams[idx] {
            Some(s) => s,
            None => return (None, ProcessResult::Ok),
        };
        if !repair_missing_timestamps(pkt, stream.reorder_delay) {
            warn!("Discard packet without pts/dts; ssrc: {}", ssrc);
            return (None, ProcessResult::Ok);
        }
        // Preserve the source timestamp in microseconds before mux timeline repair.
        let source_pts_us = unsafe { av_rescale_q(pkt.pts, stream.time_base, AV_TIME_BASE_Q) };
        let scale_global = if self.global_base_us == i64::MAX {
            0
        } else {
            source_pts_us.saturating_sub(self.global_base_us).max(0)
        };
        // println!(
        //     "pts: {}, global_base_us: {}, before diff: {}, scale_global diff: {}, tb: {:?}",
        //     pts, self.global_base_us, global, scale_global, stream.time_base
        // );
        let res = stream.process(pkt, ssrc);
        if global_base_us != i64::MAX {
            let base = unsafe { av_rescale_q(global_base_us, AV_TIME_BASE_Q, stream.time_base) };
            pkt.pts = pkt.pts.saturating_sub(base);
            pkt.dts = pkt.dts.saturating_sub(base);
        }

        (Some(scale_global), res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsmpeg::ffi::{
        AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_H264, AVMediaType_AVMEDIA_TYPE_AUDIO,
        AVMediaType_AVMEDIA_TYPE_VIDEO,
    };

    fn packet(timestamp: i64, payload: &mut u8) -> AVPacket {
        let mut packet = unsafe { std::mem::zeroed::<AVPacket>() };
        packet.stream_index = 0;
        packet.pts = timestamp;
        packet.dts = timestamp;
        packet.data = payload;
        packet.size = 1;
        packet
    }

    #[test]
    fn rebases_large_live_timestamps_before_muxing() {
        let time_base = AVRational {
            num: 1,
            den: 90_000,
        };
        let source_base = 3_712_587_622_i64;
        let mut normalizer = TimelineNormalizer::new(1);
        normalizer.init_stream(
            0,
            AVMediaType_AVMEDIA_TYPE_VIDEO,
            time_base,
            AVCodecID_AV_CODEC_ID_H264,
            0,
        );
        normalizer.rescale_global_base_us(0, source_base);

        let mut payload = 0_u8;
        let mut first = packet(source_base, &mut payload);
        assert_eq!(normalizer.process(&mut first, 1).1, ProcessResult::Ok);
        assert!((0..=1).contains(&first.dts));
        assert!((0..=1).contains(&first.pts));

        let mut next = packet(source_base + 3_600, &mut payload);
        assert_eq!(normalizer.process(&mut next, 1).1, ProcessResult::Ok);
        assert!((3_600..=3_601).contains(&next.dts));
        assert!((3_600..=3_601).contains(&next.pts));
        assert!(next.dts < i64::from(i32::MAX));
    }

    #[test]
    fn initializes_timeline_for_late_discovered_stream() {
        let time_base = AVRational { num: 1, den: 8_000 };
        let mut normalizer = TimelineNormalizer::new(1);
        normalizer.init_stream(
            1,
            AVMediaType_AVMEDIA_TYPE_AUDIO,
            time_base,
            AVCodecID_AV_CODEC_ID_AAC,
            0,
        );
        normalizer.rescale_global_base_us(1, 8_000);

        let mut payload = 0_u8;
        let mut packet = packet(8_000, &mut payload);
        packet.stream_index = 1;

        assert_eq!(normalizer.process(&mut packet, 1).1, ProcessResult::Ok);
        assert_eq!(packet.pts, 0);
        assert_eq!(packet.dts, 0);
    }
}
