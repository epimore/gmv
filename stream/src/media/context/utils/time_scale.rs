use rsmpeg::avutil::AVRational;
use rsmpeg::ffi::{
    AV_NOPTS_VALUE, AV_TIME_BASE_Q, AVMediaType, AVMediaType_AVMEDIA_TYPE_AUDIO, AVPacket,
    av_rescale_q,
};

const MAX_JUMP_US: i64 = 5_000_000; // 5s
const DEFAULT_AUDIO_DELTA: i64 = 1024;
const DEFAULT_VIDEO_DELTA: i64 = 1;
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
    last_dts: i64,
    last_pts: i64,
    normal_delta: i64,
    initialized: bool,
    stream_type: AVMediaType,
    time_base: AVRational,
}

impl StreamTimeline {
    pub fn new(stream_type: AVMediaType, time_base: AVRational) -> Self {
        Self {
            last_dts: 0,
            last_pts: 0,
            normal_delta: 0,
            initialized: false,
            stream_type,
            time_base,
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

    pub fn process(&mut self, pkt: &mut AVPacket) -> ProcessResult {
        // ===== 初始化 =====
        if !self.initialized {
            self.last_dts = pkt.dts;
            self.last_pts = pkt.pts;
            self.initialized = true;
            return ProcessResult::Ok;
        }

        let mut result = ProcessResult::Ok;

        let dts_diff = pkt.dts - self.last_dts;

        // ===== discontinuity 检测 =====
        if dts_diff <= 0 || dts_diff > MAX_DELTA_TICKS {
            result = ProcessResult::Discontinuity;
            println!("-----Discontinuity: current dts: {}, last dts: {}",pkt.dts,self.last_dts);
            // 强制单调递增
            let delta = self.get_delta();
            pkt.dts = self.last_dts + delta;
            pkt.pts = pkt.dts;
            println!("-----Discontinuity new: current dts: {}, last dts: {}",pkt.dts,self.last_dts);
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

    pub fn init_stream(&mut self, idx: usize, m_tp: AVMediaType, time_base: AVRational) {
        self.streams[idx] = Some(StreamTimeline::new(m_tp, time_base));
    }

    pub fn rescale_global_base_us(&mut self, pts: i64) {
        if pts != AV_NOPTS_VALUE {
            self.global_base_us = self.global_base_us.min(pts);
        }
    }

    pub fn process(&mut self, pkt: &mut AVPacket) -> (Option<i64>, ProcessResult) {
        let idx = pkt.stream_index as usize;

        let stream = match &mut self.streams[idx] {
            Some(s) => s,
            None => return (None, ProcessResult::Ok),
        };

        let pts = if pkt.pts != AV_NOPTS_VALUE {
            pkt.pts
        } else {
            pkt.dts
        };

        let global = pts - self.global_base_us;
        let mut scale_global = 0;
        if global > 0 {
            scale_global = unsafe { av_rescale_q(global, stream.time_base, AV_TIME_BASE_Q) };
        }
        // println!(
        //     "pts: {}, global_base_us: {}, before diff: {}, scale_global diff: {}, tb: {:?}",
        //     pts, self.global_base_us, global, scale_global, stream.time_base
        // );
        let res = stream.process(pkt);

        (Some(scale_global), res)
    }
}
