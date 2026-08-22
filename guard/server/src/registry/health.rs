use crate::core::{HealthState, SchedulingState};

pub fn scheduling_for_health(health: HealthState) -> SchedulingState {
    match health {
        HealthState::Ready => SchedulingState::Enabled,
        HealthState::Starting
        | HealthState::Degraded
        | HealthState::Draining
        | HealthState::Offline => SchedulingState::Disabled,
    }
}
