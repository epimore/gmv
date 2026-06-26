pub const API_PREFIX: &str = "/api/v2";
pub const AUTH: &str = "/api/v2/auth";
pub const DASHBOARD: &str = "/api/v2/dashboard";
pub const NODES: &str = "/api/v2/nodes";
pub const LEASES: &str = "/api/v2/leases";
pub const EVENTS: &str = "/api/v2/events";
pub const OPERATIONS: &str = "/api/v2/operations";
pub const SYSTEM_JOBS: &str = "/api/v2/system/jobs";
pub const SSE_EVENTS_STREAM: &str = "/api/v2/events/stream";

pub fn is_v2_path(path: &str) -> bool {
    path == API_PREFIX || path.starts_with("/api/v2/")
}
