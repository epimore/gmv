use rsmpeg::ffi::AVPacket;

pub struct ScaleTimeBase {
    last_dts_in: i64,      // 上次输入DTS（原始值）
    last_dts_out: i64,     // 上次输出DTS（修正后）
    last_pts_out: i64,     // 上次输出PTS（修正后）
    last_delta: i64,       // 上次帧间隔
    last_stamp: u64,       // 上次系统时间戳
    base_dts: Option<i64>, // 基准DTS（用于跳跃检测）
    base_pts: Option<i64>, // 基准PTS
    stream_type: StreamType, // 流类型（视频/音频）
    dts_wrapped: bool,     // 是否发生过回退（用于流起始阶段）
}

enum StreamType {
    Video,
    Audio,
}

impl ScaleTimeBase {
    pub fn new(stream_type: StreamType) -> Self {
        Self {
            last_dts_in: i64::MIN,
            last_dts_out: i64::MIN,
            last_pts_out: i64::MIN,
            last_delta: 0,
            last_stamp: 0,
            base_dts: None,
            base_pts: None,
            stream_type,
            dts_wrapped: false,
        }
    }

    pub fn scale(&mut self, pkt: &mut AVPacket, current_stamp: u64) {
        // 1. 初始化基准值
        if self.base_dts.is_none() {
            self.base_dts = Some(pkt.dts);
            self.base_pts = Some(pkt.pts);
            self.last_dts_in = pkt.dts;
            self.last_dts_out = pkt.dts;
            self.last_pts_out = pkt.pts;
            self.last_stamp = current_stamp;
            return;
        }

        // 2. 检测异常情况
        let dts_jump_back = pkt.dts < self.last_dts_out;
        let dts_jump_forward = pkt.dts > self.last_dts_in + self.last_delta * 10; // 超过10倍正常间隔
        let time_gap = current_stamp.saturating_sub(self.last_stamp) > 1000; // 超过1秒无数据

        // 3. 判断是否需要重新同步
        if dts_jump_back || (dts_jump_forward && time_gap) {
            self.handle_discontinuity(pkt, current_stamp);
            return;
        }

        // 4. 正常处理：更新帧间隔（用于可变帧率）
        if pkt.dts > self.last_dts_in {
            let delta = pkt.dts - self.last_dts_in;
            // 使用移动平均，避免单次抖动影响
            if self.last_delta == 0 {
                self.last_delta = delta;
            } else {
                self.last_delta = (self.last_delta * 3 + delta) / 4;
            }
        }

        // 5. 对于音频，确保PTS = DTS（音频通常如此）
        if matches!(self.stream_type, StreamType::Audio) {
            pkt.pts = pkt.dts;
        }

        // 6. 更新状态
        self.last_dts_in = pkt.dts;
        self.last_dts_out = pkt.dts;
        self.last_pts_out = pkt.pts;
        self.last_stamp = current_stamp;
    }

    fn handle_discontinuity(&mut self, pkt: &mut AVPacket, current_stamp: u64) {
        // 检测是否为正常的B帧PTS回退（显示顺序）
        let is_b_frame_pts_rollback = pkt.pts < self.last_pts_out && pkt.dts > self.last_dts_out;

        if is_b_frame_pts_rollback {
            // B帧场景：PTS回退是正常的，不需要修正
            self.last_pts_out = pkt.pts;
            self.last_dts_out = pkt.dts;
            self.last_dts_in = pkt.dts;
            self.last_stamp = current_stamp;
            return;
        }

        // 真正的异常：DTS回退或跳跃
        // 策略1：如果这是流的起始阶段（刚发生过一次回退），可能是SEek导致的，直接重置基准
        if !self.dts_wrapped {
            // 第一次回退：可能是SEEK，重置基准
            self.base_dts = Some(pkt.dts);
            self.base_pts = Some(pkt.pts);
            self.dts_wrapped = true;
        } else {
            // 第二次回退：尝试修正时间戳，保持连续性
            if let (Some(base_dts), Some(base_pts)) = (self.base_dts, self.base_pts) {
                // 计算相对于基准的偏移，并平滑处理
                let dts_offset = pkt.dts - base_dts;
                let pts_offset = pkt.pts - base_pts;

                // 使用移动平均后的last_delta来保证平滑
                pkt.dts = self.last_dts_out + self.last_delta;
                pkt.pts = pkt.dts + (pts_offset - dts_offset);
            }
        }

        self.last_dts_in = pkt.dts;
        self.last_dts_out = pkt.dts;
        self.last_pts_out = pkt.pts;
        self.last_stamp = current_stamp;
    }
}