#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Disconnected,
    Connected,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthState {
    Starting,
    Ready,
    Degraded,
    Draining,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchedulingState {
    Enabled,
    Disabled,
    TimeUnsynced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeaseState {
    Allocated,
    Confirmed,
    Failed,
    Released,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteState {
    Allocated,
    Running,
    Reconciling,
    Closed,
    Orphaned,
    Conflict,
}
