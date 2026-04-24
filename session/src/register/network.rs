use std::sync::Arc;
use std::time::Duration;
use base::dashmap::DashMap;
use base::net::state::Association;
use base::tokio::time::Instant;

//transport
pub struct Network{
    pub session: DashMap<Arc<str>, DeviceSession>,
    pub net_device_map:DashMap<Association,Arc<str>>
}
pub struct DeviceSession {
    pub contact_uri: String, // 来自 REGISTER Contact
    pub association: Association,
    pub support_lr: bool,         // Contact 是否有 lr
    pub expires: Duration,        //心跳有效期
    pub timeout_instant: Instant, //下个过期时刻
}