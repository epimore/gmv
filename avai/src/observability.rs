use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ACTUAL_MODEL_CAPACITY: usize = 16;
const PRELOAD_BUCKETS_MS: [u64; 6] = [100, 500, 1_000, 5_000, 10_000, 30_000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActualModelIdentity {
    pub model_id: String,
    pub version: String,
    pub revision: String,
    pub runtime: String,
}

impl ActualModelIdentity {
    pub(crate) fn metric_value(&self) -> String {
        format!(
            "{}@{}#{}:{}",
            self.model_id, self.version, self.revision, self.runtime
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug)]
struct ActualModelSlot {
    identity: ActualModelIdentity,
    succeeded: u64,
    failed: u64,
    cancelled: u64,
}

impl ActualModelSlot {
    fn increment(&mut self, outcome: TaskTerminalOutcome) {
        let counter = match outcome {
            TaskTerminalOutcome::Succeeded => &mut self.succeeded,
            TaskTerminalOutcome::Failed => &mut self.failed,
            TaskTerminalOutcome::Cancelled => &mut self.cancelled,
        };
        *counter = counter.saturating_add(1);
    }
}

#[derive(Debug)]
pub struct Observability {
    process_start_epoch_ms: u64,
    installed_models: AtomicUsize,
    ready_models: AtomicUsize,
    preload_count: AtomicU64,
    preload_sum_ms: AtomicU64,
    preload_buckets: [AtomicU64; PRELOAD_BUCKETS_MS.len()],
    self_test_failures: AtomicU64,
    activation_failures: AtomicU64,
    actual_model_overflow: AtomicU64,
    tasks_without_actual_model: AtomicU64,
    actual_models: Mutex<Vec<ActualModelSlot>>,
}

impl Default for Observability {
    fn default() -> Self {
        Self::new()
    }
}

impl Observability {
    pub fn new() -> Self {
        let process_start_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        Self::new_at(process_start_epoch_ms)
    }

    fn new_at(process_start_epoch_ms: u64) -> Self {
        Self {
            process_start_epoch_ms,
            installed_models: AtomicUsize::new(0),
            ready_models: AtomicUsize::new(0),
            preload_count: AtomicU64::new(0),
            preload_sum_ms: AtomicU64::new(0),
            preload_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            self_test_failures: AtomicU64::new(0),
            activation_failures: AtomicU64::new(0),
            actual_model_overflow: AtomicU64::new(0),
            tasks_without_actual_model: AtomicU64::new(0),
            actual_models: Mutex::new(Vec::with_capacity(ACTUAL_MODEL_CAPACITY)),
        }
    }

    pub fn set_installed_models(&self, count: usize) {
        self.installed_models.store(count, Ordering::Release);
    }

    pub fn set_ready_models(&self, count: usize) {
        self.ready_models.store(count, Ordering::Release);
    }

    pub fn observe_preload(&self, elapsed: Duration) {
        let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.preload_count.fetch_add(1, Ordering::AcqRel);
        saturating_add(&self.preload_sum_ms, elapsed_ms);
        for (limit, counter) in PRELOAD_BUCKETS_MS.iter().zip(&self.preload_buckets) {
            if elapsed_ms <= *limit {
                counter.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    pub fn observe_self_test_failure(&self) {
        self.self_test_failures.fetch_add(1, Ordering::AcqRel);
    }

    pub fn observe_activation_failure(&self) {
        self.activation_failures.fetch_add(1, Ordering::AcqRel);
    }

    pub fn observe_task_terminal(
        &self,
        actual_model: Option<ActualModelIdentity>,
        outcome: TaskTerminalOutcome,
    ) {
        let Some(identity) = actual_model else {
            self.tasks_without_actual_model
                .fetch_add(1, Ordering::AcqRel);
            return;
        };
        let mut slots = self
            .actual_models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(slot) = slots.iter_mut().find(|slot| slot.identity == identity) {
            slot.increment(outcome);
            return;
        }
        if slots.len() == ACTUAL_MODEL_CAPACITY {
            drop(slots);
            self.actual_model_overflow.fetch_add(1, Ordering::AcqRel);
            base::log::warn!(
                "AVAI task telemetry capacity exceeded: action=ai_task, stage=terminal, outcome=rejected, error_code=telemetry_actual_model_capacity"
            );
            return;
        }
        let mut slot = ActualModelSlot {
            identity,
            succeeded: 0,
            failed: 0,
            cancelled: 0,
        };
        slot.increment(outcome);
        slots.push(slot);
    }

    pub fn snapshot(&self) -> HashMap<String, String> {
        let mut metrics = HashMap::with_capacity(17 + ACTUAL_MODEL_CAPACITY * 4);
        insert_atomic_usize(&mut metrics, "installed_models", &self.installed_models);
        insert_atomic_usize(&mut metrics, "ready_models", &self.ready_models);
        metrics.insert(
            "telemetry_process_start_epoch_ms".to_string(),
            self.process_start_epoch_ms.to_string(),
        );
        insert_atomic_u64(&mut metrics, "preload_seconds_count", &self.preload_count);
        insert_atomic_u64(&mut metrics, "preload_seconds_sum_ms", &self.preload_sum_ms);
        for (name, counter) in [
            "preload_seconds_le_100ms",
            "preload_seconds_le_500ms",
            "preload_seconds_le_1s",
            "preload_seconds_le_5s",
            "preload_seconds_le_10s",
            "preload_seconds_le_30s",
        ]
        .into_iter()
        .zip(&self.preload_buckets)
        {
            insert_atomic_u64(&mut metrics, name, counter);
        }
        insert_atomic_u64(&mut metrics, "preload_seconds_le_inf", &self.preload_count);
        insert_atomic_u64(
            &mut metrics,
            "self_test_failures_total",
            &self.self_test_failures,
        );
        insert_atomic_u64(
            &mut metrics,
            "activation_failures_total",
            &self.activation_failures,
        );
        insert_atomic_u64(
            &mut metrics,
            "tasks_actual_model_overflow_total",
            &self.actual_model_overflow,
        );
        insert_atomic_u64(
            &mut metrics,
            "tasks_without_actual_model_total",
            &self.tasks_without_actual_model,
        );
        let slots = self
            .actual_models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (index, slot) in slots.iter().enumerate() {
            let prefix = format!("tasks_actual_model_slot_{index:02}");
            metrics.insert(format!("{prefix}_identity"), slot.identity.metric_value());
            metrics.insert(format!("{prefix}_succeeded"), slot.succeeded.to_string());
            metrics.insert(format!("{prefix}_failed"), slot.failed.to_string());
            metrics.insert(format!("{prefix}_cancelled"), slot.cancelled.to_string());
        }
        metrics
    }
}

fn saturating_add(counter: &AtomicU64, value: u64) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(value))
    });
}

fn insert_atomic_u64(metrics: &mut HashMap<String, String>, key: &str, value: &AtomicU64) {
    metrics.insert(key.to_string(), value.load(Ordering::Acquire).to_string());
}

fn insert_atomic_usize(metrics: &mut HashMap<String, String>, key: &str, value: &AtomicUsize) {
    metrics.insert(key.to_string(), value.load(Ordering::Acquire).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(index: usize) -> ActualModelIdentity {
        ActualModelIdentity {
            model_id: format!("model-{index}"),
            version: "1".to_string(),
            revision: format!("rev-{index}"),
            runtime: "fake".to_string(),
        }
    }

    #[test]
    fn snapshot_is_fixed_key_bounded_and_process_epoch_scoped() {
        let telemetry = Observability::new_at(100);
        telemetry.set_installed_models(3);
        telemetry.set_ready_models(2);
        telemetry.observe_preload(Duration::from_millis(500));
        telemetry.observe_self_test_failure();
        telemetry.observe_activation_failure();
        telemetry.observe_task_terminal(None, TaskTerminalOutcome::Failed);
        for index in 0..ACTUAL_MODEL_CAPACITY {
            telemetry.observe_task_terminal(Some(identity(index)), TaskTerminalOutcome::Succeeded);
        }
        telemetry.observe_task_terminal(Some(identity(16)), TaskTerminalOutcome::Cancelled);
        telemetry.observe_task_terminal(Some(identity(0)), TaskTerminalOutcome::Failed);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot["telemetry_process_start_epoch_ms"], "100");
        assert_eq!(snapshot["installed_models"], "3");
        assert_eq!(snapshot["ready_models"], "2");
        assert_eq!(snapshot["preload_seconds_count"], "1");
        assert_eq!(snapshot["preload_seconds_le_500ms"], "1");
        assert_eq!(snapshot["preload_seconds_le_100ms"], "0");
        assert_eq!(snapshot["self_test_failures_total"], "1");
        assert_eq!(snapshot["activation_failures_total"], "1");
        assert_eq!(snapshot["tasks_without_actual_model_total"], "1");
        assert_eq!(snapshot["tasks_actual_model_overflow_total"], "1");
        assert_eq!(snapshot["tasks_actual_model_slot_00_succeeded"], "1");
        assert_eq!(snapshot["tasks_actual_model_slot_00_failed"], "1");
        assert_eq!(snapshot["tasks_actual_model_slot_15_succeeded"], "1");
        assert!(!snapshot.values().any(|value| value.contains("model-16")));
        assert!(snapshot.len() <= 17 + ACTUAL_MODEL_CAPACITY * 4);

        let restarted = Observability::new_at(200).snapshot();
        assert_eq!(restarted["telemetry_process_start_epoch_ms"], "200");
        assert_eq!(restarted["installed_models"], "0");
        assert_eq!(restarted["preload_seconds_count"], "0");
        assert!(!restarted.keys().any(|key| key.contains("slot_00")));
    }
}
