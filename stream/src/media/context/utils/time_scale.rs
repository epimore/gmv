use rsmpeg::ffi::{
    av_rescale_q, AV_NOPTS_VALUE, AV_TIME_BASE_Q, AVMediaType, AVMediaType_AVMEDIA_TYPE_AUDIO,
    AVMediaType_AVMEDIA_TYPE_SUBTITLE, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket, AVRational,
};

use std::cmp;
use std::collections::{VecDeque, HashSet};
use std::time::{Instant, Duration};

// ============================================================================
// 常量定义
// ============================================================================

/// Delta 历史记录大小（用于中位数计算）
const DELTA_HISTORY_SIZE: usize = 16;

/// 最大允许的跳跃时间（微秒），超过此值视为时间线断裂
const MAX_JUMP_US: i64 = 5_000_000; // 5 秒

/// 最小允许的 Delta（tick 数），避免学习到 0 或负值
const MIN_DELTA_TICKS: i64 = 1;

/// 音频默认帧大小（当无法从 codec 获取时使用）
const DEFAULT_AUDIO_FRAME_SIZE: i64 = 1024;

/// 视频最小帧间隔（tick 数），用于防止过小的 delta
const MIN_VIDEO_DELTA_TICKS: i64 = 1;

/// 音频最大合理帧间隔（tick 数）
const MAX_AUDIO_DELTA_TICKS: i64 = 100_000;

/// 视频最大合理帧间隔（tick 数）
const MAX_VIDEO_DELTA_TICKS: i64 = 500_000;

/// 最大允许的 Delta（tick 数），防止学习到异常大的值
const MAX_DELTA_TICKS: i64 = 500_000;

/// 全局基准最大等待时间（秒）
const MAX_GLOBAL_BASE_WAIT_SECS: u64 = 5;

/// 最大等待包数（用于强制完成）
const MAX_WAIT_PACKETS: usize = 500;

// ============================================================================
// 处理结果枚举
// ============================================================================

/// 流时间线处理结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessResult {
    /// 正常处理
    Ok,
    /// 检测到时间线断裂（跳跃），需要外层处理（如清空解码器缓存）
    Discontinuity,
}

// ============================================================================
// 流级别的时间线归一化器
// ============================================================================

/// 单个流的时间线管理器
#[derive(Debug, Clone)]
pub struct StreamTimeline {
    /// 原始基准 DTS（用于归一化）
    base_dts: i64,
    /// 原始基准 PTS（用于归一化）
    base_pts: i64,

    /// 上一个包的归一化 DTS
    last_dts: i64,
    /// 上一个包的归一化 PTS
    last_pts: i64,

    /// 学习到的正常帧间隔（归一化后的 tick 数）
    normal_delta: i64,
    /// Delta 历史记录（用于计算中位数）
    delta_history: VecDeque<i64>,

    /// 流类型
    stream_type: AVMediaType,
    /// 是否已初始化
    initialized: bool,

    /// 第一个包的原始 PTS（微秒），用于全局同步
    first_pts_us: Option<i64>,
    /// 是否已经用于计算全局基准
    has_contributed_to_global_base: bool,
}

impl StreamTimeline {
    /// 创建新的流时间线管理器
    pub fn new(stream_type: AVMediaType) -> Self {
        Self {
            base_dts: 0,
            base_pts: 0,
            last_dts: 0,
            last_pts: 0,
            normal_delta: 0,
            delta_history: VecDeque::with_capacity(DELTA_HISTORY_SIZE),
            stream_type,
            initialized: false,
            first_pts_us: None,
            has_contributed_to_global_base: false,
        }
    }

    /// 将差值转换为微秒
    #[inline]
    fn diff_to_us(diff: i64, tb: AVRational) -> i64 {
        if diff == 0 {
            return 0;
        }
        unsafe { av_rescale_q(diff, tb, AV_TIME_BASE_Q) }
    }

    /// 判断是否为视频流
    fn is_video(&self) -> bool {
        self.stream_type == AVMediaType_AVMEDIA_TYPE_VIDEO
    }

    /// 判断是否为音频流
    fn is_audio(&self) -> bool {
        self.stream_type == AVMediaType_AVMEDIA_TYPE_AUDIO
    }

    /// 判断是否为字幕流
    fn is_subtitle(&self) -> bool {
        self.stream_type == AVMediaType_AVMEDIA_TYPE_SUBTITLE
    }

    /// 获取合理的默认 delta
    fn get_default_delta(&self) -> i64 {
        if self.is_video() {
            MIN_VIDEO_DELTA_TICKS
        } else if self.is_audio() {
            DEFAULT_AUDIO_FRAME_SIZE
        } else {
            1
        }
    }

    /// 获取该流类型的最大允许 delta
    fn get_max_delta(&self) -> i64 {
        if self.is_video() {
            MAX_VIDEO_DELTA_TICKS
        } else if self.is_audio() {
            MAX_AUDIO_DELTA_TICKS
        } else {
            MAX_DELTA_TICKS
        }
    }

    /// 学习 delta（使用中位数滤波，带上限保护）
    fn learn_delta(&mut self, delta: i64) {
        if delta < MIN_DELTA_TICKS {
            return;
        }

        let max_delta = self.get_max_delta();
        if delta > max_delta {
            return;
        }

        if self.normal_delta > 0 {
            let min_acceptable = self.normal_delta / 2;
            let max_acceptable = cmp::min(self.normal_delta * 2, max_delta);
            if delta < min_acceptable || delta > max_acceptable {
                return;
            }
        }

        self.delta_history.push_back(delta);
        if self.delta_history.len() > DELTA_HISTORY_SIZE {
            self.delta_history.pop_front();
        }

        let mut sorted: Vec<i64> = self.delta_history.iter().copied().collect();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        self.normal_delta = cmp::min(median, max_delta);
    }

    /// 获取第一个包的原始 PTS（微秒）
    pub fn get_first_pts_us(&self) -> Option<i64> {
        self.first_pts_us
    }

    /// 检查是否已经贡献给全局基准
    pub fn has_contributed(&self) -> bool {
        self.has_contributed_to_global_base
    }

    /// 标记为已贡献给全局基准
    pub fn mark_contributed(&mut self) {
        self.has_contributed_to_global_base = true;
    }

    /// 处理单个包
    pub fn process(&mut self, pkt: &mut AVPacket) -> (ProcessResult, Option<i64>) {
        // 记录第一个包的原始 PTS
        let mut new_first_pts = None;
        if self.first_pts_us.is_none() {
            let raw_pts = if pkt.pts != AV_NOPTS_VALUE {
                pkt.pts
            } else {
                pkt.dts
            };
            let raw_pts_us = unsafe {
                av_rescale_q(raw_pts, pkt.time_base, AV_TIME_BASE_Q)
            };
            self.first_pts_us = Some(raw_pts_us);
            new_first_pts = Some(raw_pts_us);
        }

        // 初始化
        if !self.initialized {
            self.base_dts = pkt.dts;
            self.base_pts = if pkt.pts != AV_NOPTS_VALUE {
                pkt.pts
            } else {
                pkt.dts
            };

            pkt.dts = 0;
            pkt.pts = 0;

            self.last_dts = 0;
            self.last_pts = 0;
            self.initialized = true;

            return (ProcessResult::Ok, new_first_pts);
        }

        // 保存原始值（用于跳跃检测）
        let orig_dts = pkt.dts;
        let orig_pts = pkt.pts;

        // 归一化
        pkt.dts -= self.base_dts;
        if pkt.pts != AV_NOPTS_VALUE {
            pkt.pts -= self.base_pts;
        }

        // 补齐缺失的 PTS
        if pkt.pts == AV_NOPTS_VALUE {
            if self.is_audio() || self.is_subtitle() || self.is_video() {
                pkt.pts = pkt.dts;
            }
        }

        // 计算差值
        let tb = pkt.time_base;
        let dts_diff = pkt.dts - self.last_dts;
        let pts_diff = if pkt.pts != AV_NOPTS_VALUE {
            pkt.pts - self.last_pts
        } else {
            0
        };

        let dts_diff_us = Self::diff_to_us(dts_diff, tb);

        // 检测时间线断裂
        if dts_diff_us < 0 || dts_diff_us > MAX_JUMP_US {
            let jump_dts = orig_dts;
            let jump_pts = if orig_pts != AV_NOPTS_VALUE {
                orig_pts
            } else {
                orig_dts
            };

            self.base_dts = jump_dts;
            self.base_pts = jump_pts;

            pkt.dts = 0;
            pkt.pts = 0;

            self.last_dts = 0;
            self.last_pts = 0;
            self.normal_delta = 0;
            self.delta_history.clear();

            return (ProcessResult::Discontinuity, None);
        }

        // 修复 DTS 异常
        if dts_diff <= 0 {
            let delta = if pkt.duration > 0 && pkt.duration < self.get_max_delta() {
                pkt.duration
            } else if self.normal_delta > 0 {
                self.normal_delta
            } else {
                self.get_default_delta()
            };
            pkt.dts = self.last_dts + delta;
        }

        // 修复 PTS 异常
        if pkt.pts != AV_NOPTS_VALUE {
            if self.is_video() {
                if pts_diff < 0 {
                    let min_pts_diff = -self.get_default_delta() * 2;
                    if pts_diff < min_pts_diff {
                        pkt.pts = pkt.dts;
                    }
                }
            } else {
                if pts_diff <= 0 {
                    let delta = if self.normal_delta > 0 {
                        self.normal_delta
                    } else {
                        self.get_default_delta()
                    };
                    pkt.pts = self.last_pts + delta;
                }
            }
        }

        // 学习 Delta
        if !self.is_subtitle() {
            let delta = pkt.dts - self.last_dts;
            if delta >= MIN_DELTA_TICKS {
                self.learn_delta(delta);
            }
        }

        // 更新状态
        self.last_dts = pkt.dts;
        if pkt.pts != AV_NOPTS_VALUE {
            self.last_pts = pkt.pts;
        }

        (ProcessResult::Ok, None)
    }

    /// 重置时间线
    pub fn reset(&mut self) {
        self.initialized = false;
        self.base_dts = 0;
        self.base_pts = 0;
        self.last_dts = 0;
        self.last_pts = 0;
        self.normal_delta = 0;
        self.delta_history.clear();
        self.first_pts_us = None;
        self.has_contributed_to_global_base = false;
    }

    /// 获取当前的归一化基准
    pub fn get_base(&self) -> (i64, i64) {
        (self.base_dts, self.base_pts)
    }

    /// 获取学习的正常 delta
    pub fn get_normal_delta(&self) -> i64 {
        self.normal_delta
    }
}

// ============================================================================
// 全局时间线归一化器
// ============================================================================

/// 全局时间线管理器
pub struct TimelineNormalizer {
    /// 各流的时间线管理器（索引对应 stream index）
    streams: Vec<Option<StreamTimeline>>,

    /// 所有需要等待初始化的流索引
    pending_streams: HashSet<usize>,

    /// 必须出现的流类型（用于判断哪些流是必要的）
    required_stream_types: HashSet<AVMediaType>,

    /// 实际出现的流索引
    active_streams: HashSet<usize>,

    /// 全局基准时间（微秒）
    global_base_us: Option<i64>,

    /// 是否已经确定了全局基准
    global_base_fixed: bool,

    /// 收集到的所有流的第一帧 PTS（微秒）
    collected_first_pts: Vec<Option<i64>>,

    /// 开始等待的时间点（用于超时检测）
    wait_start_time: Option<Instant>,

    /// 已强制完成的流索引
    forced_completed: HashSet<usize>,

    /// 全局主时钟（微秒）
    pub master_clock_us: i64,
}

impl TimelineNormalizer {
    /// 创建新的全局时间线管理器
    pub fn new(stream_count: usize) -> Self {
        let mut pending_streams = HashSet::new();
        for i in 0..stream_count {
            pending_streams.insert(i);
        }

        // 默认必须出现的流类型：视频和音频
        let mut required_stream_types = HashSet::new();
        required_stream_types.insert(AVMediaType_AVMEDIA_TYPE_VIDEO);
        required_stream_types.insert(AVMediaType_AVMEDIA_TYPE_AUDIO);

        Self {
            streams: (0..stream_count).map(|_| None).collect(),
            pending_streams,
            required_stream_types,
            active_streams: HashSet::new(),
            global_base_us: None,
            global_base_fixed: false,
            collected_first_pts: vec![None; stream_count],
            wait_start_time: None,
            forced_completed: HashSet::new(),
            master_clock_us: 0,
        }
    }

    /// 设置必须出现的流类型
    pub fn set_required_stream_types(&mut self, required: HashSet<AVMediaType>) {
        self.required_stream_types = required;
    }

    /// 初始化流
    pub fn init_stream(&mut self, idx: usize, media_type: AVMediaType) {
        if idx < self.streams.len() {
            self.streams[idx] = Some(StreamTimeline::new(media_type));
        }
    }

    /// 记录流的第一帧 PTS
    pub fn record_first_pts(&mut self, idx: usize, first_pts_us: i64) {
        if idx < self.collected_first_pts.len() {
            self.collected_first_pts[idx] = Some(first_pts_us);
            self.active_streams.insert(idx);
            self.pending_streams.remove(&idx);

            // 开始计时
            if self.wait_start_time.is_none() && !self.pending_streams.is_empty() {
                self.wait_start_time = Some(Instant::now());
            }

            self.try_fix_global_base();
        }
    }

    /// 标记某个流不再需要等待
    pub fn mark_stream_complete(&mut self, idx: usize) {
        if !self.forced_completed.contains(&idx) {
            self.forced_completed.insert(idx);

            // 如果该流还没有第一帧 PTS，使用当前已收集的最大值作为默认
            if self.collected_first_pts[idx].is_none() {
                let max_pts = self.collected_first_pts
                    .iter()
                    .filter_map(|&p| p)
                    .max()
                    .unwrap_or(0);
                self.collected_first_pts[idx] = Some(max_pts);
                log::info!(
                    "Stream {} forced complete with default PTS: {}",
                    idx, max_pts
                );
            }

            self.pending_streams.remove(&idx);
            self.try_fix_global_base();
        }
    }

    /// 检查超时并强制完成
    fn check_timeout(&mut self) {
        if self.global_base_fixed {
            return;
        }

        if let Some(start_time) = self.wait_start_time {
            if start_time.elapsed() >= Duration::from_secs(MAX_GLOBAL_BASE_WAIT_SECS) {
                // 超时，强制完成所有未出现的流
                let remaining: Vec<usize> = self.pending_streams.iter().copied().collect();
                for idx in remaining {
                    log::info!(
                        "Stream {} timeout after {} seconds, marking as complete",
                        idx, MAX_GLOBAL_BASE_WAIT_SECS
                    );
                    self.mark_stream_complete(idx);
                }
            }
        }
    }

    /// 尝试确定全局基准
    fn try_fix_global_base(&mut self) {
        if self.global_base_fixed {
            return;
        }

        // 检查是否还有必要的流未出现
        let pending_required = self.pending_streams.iter().any(|&idx| {
            if let Some(stream) = &self.streams[idx] {
                self.required_stream_types.contains(&stream.stream_type)
            } else {
                false
            }
        });

        if pending_required {
            return;
        }

        // 收集所有有效的第一帧 PTS
        let all_first_pts: Vec<i64> = self.collected_first_pts
            .iter()
            .filter_map(|&pts| pts)
            .collect();

        if all_first_pts.is_empty() {
            return;
        }

        // 计算最小时间戳作为全局基准
        let min_pts = *all_first_pts.iter().min().unwrap();
        self.global_base_us = Some(min_pts);
        self.global_base_fixed = true;

        log::info!(
            "Global base fixed: {} us (using {} streams)",
            min_pts,
            all_first_pts.len()
        );

        // 标记所有流已贡献
        for stream_opt in &mut self.streams {
            if let Some(stream) = stream_opt {
                stream.mark_contributed();
            }
        }
    }

    /// 处理单个包
    pub fn process(&mut self, pkt: &mut AVPacket) -> (Option<i64>, ProcessResult) {
        // 每次处理前检查超时
        self.check_timeout();

        if pkt.stream_index < 0 {
            return (None, ProcessResult::Ok);
        }

        let idx = pkt.stream_index as usize;
        if idx >= self.streams.len() {
            return (None, ProcessResult::Ok);
        }

        let stream = match &mut self.streams[idx] {
            Some(s) => s,
            None => return (None, ProcessResult::Ok),
        };

        let (stream_result, first_pts) = stream.process(pkt);

        // 如果是第一次遇到这个流且全局基准还未固定，记录第一帧
        if let Some(first_pts_us) = first_pts {
            if !self.global_base_fixed {
                self.record_first_pts(idx, first_pts_us);
            }
        }

        // 如果全局基准还未确定，返回 None
        if !self.global_base_fixed {
            return (None, stream_result);
        }

        // 获取流内时间戳
        let stream_pts = if pkt.pts != AV_NOPTS_VALUE {
            pkt.pts
        } else {
            pkt.dts
        };

        let stream_pts_us = unsafe { av_rescale_q(stream_pts, pkt.time_base, AV_TIME_BASE_Q) };

        // 计算全局时间戳
        let global_base_us = self.global_base_us.unwrap();
        let global_pts_us = stream_pts_us - global_base_us;

        (Some(global_pts_us), stream_result)
    }

    /// 重置所有流的时间线
    pub fn reset_all(&mut self) {
        for stream in self.streams.iter_mut().flatten() {
            stream.reset();
        }
        self.global_base_us = None;
        self.global_base_fixed = false;
        self.collected_first_pts.fill(None);
        self.active_streams.clear();
        self.forced_completed.clear();
        self.pending_streams.clear();
        for i in 0..self.streams.len() {
            self.pending_streams.insert(i);
        }
        self.wait_start_time = None;
    }

    /// 重置指定流的时间线
    pub fn reset_stream(&mut self, idx: usize) {
        if idx < self.streams.len() {
            if let Some(stream) = &mut self.streams[idx] {
                stream.reset();
            }
            self.pending_streams.insert(idx);
            self.collected_first_pts[idx] = None;
            self.active_streams.remove(&idx);
            self.forced_completed.remove(&idx);
        }
        self.global_base_us = None;
        self.global_base_fixed = false;
        self.wait_start_time = None;
    }

    /// 获取全局基准（微秒）
    pub fn get_global_base(&self) -> Option<i64> {
        self.global_base_us
    }

    /// 检查全局基准是否已确定
    pub fn is_global_base_fixed(&self) -> bool {
        self.global_base_fixed
    }

    /// 获取待处理的流数量
    pub fn pending_streams_count(&self) -> usize {
        self.pending_streams.len()
    }

    /// 获取活跃流数量
    pub fn active_streams_count(&self) -> usize {
        self.active_streams.len()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rsmpeg::ffi::{AVRational, AVPacket};

    fn create_test_packet(dts: i64, pts: i64, stream_index: i32, time_base: AVRational) -> AVPacket {
        let mut pkt = AVPacket {
            pts,
            dts,
            stream_index,
            time_base,
            duration: 0,
            ..unsafe { std::mem::zeroed() }
        };
        pkt
    }

    #[test]
    fn test_global_base_fixed_after_required_streams() {
        let mut normalizer = TimelineNormalizer::new(3);
        normalizer.init_stream(0, AVMediaType_AVMEDIA_TYPE_VIDEO);
        normalizer.init_stream(1, AVMediaType_AVMEDIA_TYPE_AUDIO);
        normalizer.init_stream(2, AVMediaType_AVMEDIA_TYPE_SUBTITLE);

        let video_tb = AVRational { num: 1, den: 24 };
        let audio_tb = AVRational { num: 1, den: 44100 };

        // 处理视频（基准未确定）
        let mut video_pkt = create_test_packet(0, 0, 0, video_tb);
        let (global1, _) = normalizer.process(&mut video_pkt);
        assert!(global1.is_none());
        assert!(!normalizer.is_global_base_fixed());

        // 处理音频（必要的流都出现了，基准应该确定）
        let mut audio_pkt = create_test_packet(-1024, -1024, 1, audio_tb);
        let (global2, _) = normalizer.process(&mut audio_pkt);
        assert!(global2.is_some());
        assert!(normalizer.is_global_base_fixed());

        // 字幕流未出现，但不应阻塞基准确定
        assert_eq!(normalizer.pending_streams_count(), 1); // 字幕流还在等待
        assert!(normalizer.is_global_base_fixed()); // 但基准已确定
    }

    #[test]
    fn test_timeout_force_complete() {
        let mut normalizer = TimelineNormalizer::new(2);
        normalizer.init_stream(0, AVMediaType_AVMEDIA_TYPE_VIDEO);
        normalizer.init_stream(1, AVMediaType_AVMEDIA_TYPE_AUDIO);

        let video_tb = AVRational { num: 1, den: 24 };

        // 只处理视频
        let mut video_pkt = create_test_packet(0, 0, 0, video_tb);
        let (global, _) = normalizer.process(&mut video_pkt);
        assert!(global.is_none());

        // 手动触发超时检查（实际场景中会由 process 自动触发）
        normalizer.check_timeout();

        // 超时后应该强制完成音频流
        assert!(normalizer.is_global_base_fixed());
    }

    #[test]
    fn test_discontinuity_detection() {
        let mut normalizer = TimelineNormalizer::new(1);
        normalizer.init_stream(0, AVMediaType_AVMEDIA_TYPE_VIDEO);

        let tb = AVRational { num: 1, den: 24 };

        // 第一个包
        let mut pkt1 = create_test_packet(0, 0, 0, tb);
        let (global1, result1) = normalizer.process(&mut pkt1);
        assert!(global1.is_some());
        assert_eq!(result1, ProcessResult::Ok);

        // 第二个包（正常间隔）
        let mut pkt2 = create_test_packet(1, 1, 0, tb);
        let (global2, result2) = normalizer.process(&mut pkt2);
        assert!(global2.is_some());
        assert_eq!(result2, ProcessResult::Ok);

        // 第三个包（时间跳跃，超过 5 秒）
        let mut pkt3 = create_test_packet(200, 200, 0, tb);
        let (global3, result3) = normalizer.process(&mut pkt3);
        assert!(global3.is_some());
        assert_eq!(result3, ProcessResult::Discontinuity);

        // 跳跃后时间戳应该被重置为 0
        assert_eq!(pkt3.dts, 0);
        assert_eq!(pkt3.pts, 0);
    }

    #[test]
    fn test_reset_all() {
        let mut normalizer = TimelineNormalizer::new(1);
        normalizer.init_stream(0, AVMediaType_AVMEDIA_TYPE_VIDEO);

        let tb = AVRational { num: 1, den: 24 };

        let mut pkt = create_test_packet(0, 0, 0, tb);
        normalizer.process(&mut pkt);

        assert!(normalizer.is_global_base_fixed());

        normalizer.reset_all();

        assert!(!normalizer.is_global_base_fixed());
        assert_eq!(normalizer.pending_streams_count(), 1);

        // 重新处理，应该重新建立基准
        let mut pkt = create_test_packet(100, 100, 0, tb);
        let (global, _) = normalizer.process(&mut pkt);
        assert!(global.is_some());
        assert!(normalizer.is_global_base_fixed());
    }

    #[test]
    fn test_mark_stream_complete() {
        let mut normalizer = TimelineNormalizer::new(2);
        normalizer.init_stream(0, AVMediaType_AVMEDIA_TYPE_VIDEO);
        normalizer.init_stream(1, AVMediaType_AVMEDIA_TYPE_AUDIO);

        let video_tb = AVRational { num: 1, den: 24 };

        // 只处理视频
        let mut video_pkt = create_test_packet(0, 0, 0, video_tb);
        let (global, _) = normalizer.process(&mut video_pkt);
        assert!(global.is_none());

        // 手动标记音频流完成
        normalizer.mark_stream_complete(1);

        // 现在应该可以确定基准
        assert!(normalizer.is_global_base_fixed());

        // 再次处理视频包，应该能获取全局时间戳
        let mut video_pkt2 = create_test_packet(1, 1, 0, video_tb);
        let (global2, _) = normalizer.process(&mut video_pkt2);
        assert!(global2.is_some());
    }
}