#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockState {
    Synced,
    Warn,
    TimeUnsynced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockClassifier {
    pub warn_offset_ms: i64,
    pub severe_offset_ms: i64,
}

impl Default for ClockClassifier {
    fn default() -> Self {
        Self {
            warn_offset_ms: 1_000,
            severe_offset_ms: 5_000,
        }
    }
}

impl ClockClassifier {
    pub fn classify(&self, estimated_offset_ms: i64) -> ClockState {
        let offset = estimated_offset_ms.abs();
        if offset >= self.severe_offset_ms {
            ClockState::TimeUnsynced
        } else if offset >= self.warn_offset_ms {
            ClockState::Warn
        } else {
            ClockState::Synced
        }
    }
}
